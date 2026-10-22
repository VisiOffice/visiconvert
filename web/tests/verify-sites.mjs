import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';
import path from 'node:path';

export async function verifySites({ browser, page, origin, api, artifactDir }) {
  let siteId;
  const siteName = `Website verification ${Date.now()}`;
  const fixture = createServer((req, res) => {
    res.setHeader('Content-Type','text/html');
    res.end(`<!doctype html><html><head><title>Capture test business</title><script defer src="${origin}/tracker.js" data-site="${siteId}" referrerpolicy="no-referrer"></script></head><body><h1>Request a quote</h1><form><input aria-label="Customer name" name="name" required><input aria-label="Customer email" name="email" type="email" required><button>Submit request</button></form><p id="status"></p><script>document.querySelector('form').addEventListener('submit',e=>{e.preventDefault();document.querySelector('#status').textContent='Form validation passed; awaiting backend confirmation';});</script></body></html>`);
  });
  fixture.listen(0,'127.0.0.1'); await once(fixture,'listening');
  const business = `http://127.0.0.1:${fixture.address().port}`;
  const visitor = await browser.newPage();
  const collected = []; const errors = [];
  visitor.on('request', r => { if (r.url().includes('/collect/')) collected.push({url:r.url(),data:r.postData()}); });
  visitor.on('pageerror', e => errors.push(e.message));
  try {
    await page.getByRole('button',{name:'Sites',exact:true}).click();
    await page.getByLabel('Website name',{exact:true}).fill(siteName);
    await page.getByLabel('Website origin',{exact:true}).fill(business);
    await page.getByRole('button',{name:'Add website',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Site saved.'}).waitFor();
    await page.getByLabel('Installation snippet').waitFor();
    const sites = (await api('/sites')).body;
    const site = sites.sites.find(s => s.name === siteName);
    siteId = site.id;
    assert((await page.getByLabel('Installation snippet').inputValue()).includes(siteId));
    await visitor.goto(`${business}/quote?utm_source=google&utm_medium=cpc&utm_campaign=website-check-${siteId}&gclid=click-123&password=private#secret`);
    await visitor.waitForFunction(()=>!!window.VisiConvert);
    assert.equal(collected.length,0,'Loading tracker must not collect before consent');
    await visitor.getByRole('button',{name:'Submit request'}).click();
    assert.equal(collected.length,0,'Validation failure must not create a lead');
    await visitor.evaluate(()=>window.VisiConvert.setConsent(true));
    await visitor.getByLabel('Customer name').fill('Website browser customer');
    await visitor.getByLabel('Customer email').fill('browser@example.com');
    await visitor.getByRole('button',{name:'Submit request'}).click();
    assert.equal(collected.filter(r=>r.url.endsWith('/forms')).length,0,'A valid submit event is still not a confirmed success');
    const capture = () => window.VisiConvert.trackForm({submissionId:'browser-success-1',fields:{name:'Website browser customer',email:'browser@example.com',password:'must-never-leave',notes:'private notes'}});
    await visitor.evaluate(capture); await visitor.evaluate(capture);
    assert(!JSON.stringify(collected).includes('must-never-leave'));
    assert(!JSON.stringify(collected).includes('private'));
    let leads = (await api('/leads')).body.filter(l=>l.name==='Website browser customer' && l.campaign===`website-check-${siteId}`);
    assert.equal(leads.length,1,'Repeated success callback must create one lead');
    assert.equal(leads[0].source,'google'); assert.equal(leads[0].campaign,`website-check-${siteId}`);
    const evidence = (await api(`/leads/${leads[0].id}/website-evidence`)).body;
    assert.equal(evidence.evidence.landing_url,`${business}/quote`);
    assert.equal(evidence.evidence.gclid,'click-123');
    await page.getByRole('button',{name:'Refresh diagnostics'}).click();
    await page.getByText('Visits received',{exact:true}).waitFor();
    const stats = (await api('/sites')).body.sites.find(s=>s.id===siteId);
    assert.equal(stats.session_count,1); assert.equal(stats.form_count,1);
    await page.screenshot({path:path.join(artifactDir,'sites-desktop.png'),fullPage:true});
    await page.setViewportSize({width:390,height:844});
    assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Sites must fit mobile');
    await page.screenshot({path:path.join(artifactDir,'sites-mobile.png'),fullPage:true});
    await page.setViewportSize({width:1440,height:1000});
    // Refresh the app's shared inbox state after the external form capture.
    await page.getByRole('button',{name:'Refresh data',exact:true}).click();
    await page.getByRole('button',{name:/Lead inbox/}).click();
    await page.getByLabel('Search leads').fill('Website browser customer');
    await page.getByRole('button',{name:/Website browser customer/}).first().click();
    await page.getByText('Website source evidence',{exact:true}).click();
    await page.getByText('click-123',{exact:true}).waitFor();
    await page.screenshot({path:path.join(artifactDir,'website-lead-evidence.png'),fullPage:true});
    await page.getByRole('button',{name:'Close details'}).click();
    await visitor.evaluate(()=>window.VisiConvert.setConsent(false));
    const before = collected.length;
    assert(await visitor.evaluate(async()=>{try{await window.VisiConvert.trackForm({submissionId:'no-consent'});return false;}catch{return true;}}));
    assert.equal(collected.length,before,'Withdrawn consent must stop requests');
    await page.getByRole('button',{name:'Sites',exact:true}).click();
    await page.getByLabel('Website',{exact:true}).selectOption(siteId);
    await page.getByRole('button',{name:'Edit website',exact:true}).click();
    await page.getByLabel('Collection enabled',{exact:true}).uncheck();
    await page.getByRole('button',{name:'Save website',exact:true}).click();
    await page.getByText('Collection paused',{exact:true}).waitFor();
    const paused = await fetch(`${origin}/collect/sites/${siteId}/config`,{headers:{Origin:business}});
    assert.equal(paused.status,404);
    assert.deepEqual(errors,[]);
  } finally {
    await visitor.close(); await new Promise(resolve=>fixture.close(resolve));
  }
}
