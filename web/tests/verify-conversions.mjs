import assert from 'node:assert/strict';
import path from 'node:path';
export const openTab=(page,name)=>page.getByRole('tab',{name:new RegExp(`^${name}`)}).click();
export async function verifyConversions({page,api,artifactDir}) {
  const lead=(await api('/leads')).body.find(l=>l.name==='Website browser customer');
  assert(lead,'Website fixture must exist');
  const inquiry=(await api(`/inquiries/${lead.inquiry_id}`)).body.inquiry;
  assert.equal((await api(`/inquiries/${inquiry.id}`,{qualification:'qualified',notes:inquiry.notes,version:inquiry.version},'PATCH')).status,200);
  const booking={external_id:'ads-browser-booking',amount_minor:12345,currency:'USD',status:'booked',revision:1};
  assert.equal((await api(`/inquiries/${inquiry.id}/outcomes`,booking,'PUT')).status,200);
  await page.getByRole('button',{name:'Conversions',exact:true}).click();
  // Tabs open on the overview; nothing steers a new workspace to the fixed reviewed actions.
  const tabs=await page.getByRole('tablist',{name:'Conversion settings'}).getByRole('tab').allInnerTexts();
  assert.deepEqual(tabs.map(t=>t.replace(/in use$/i,'').replace(/\s+/g,' ').trim()),['Overview','Actions','Google Ads','Microsoft Ads','Meta Ads','Events','Advanced']);
  assert.equal(await page.getByRole('tab',{selected:true}).innerText(),'Overview');
  await page.getByRole('article',{name:'Google Ads status',exact:true}).waitFor();
  await openTab(page,'Events');
  await page.getByRole('region',{name:'Recent pipeline events',exact:true}).waitFor();
  assert(!(await page.getByRole('button',{name:'Enable reviewed delivery',exact:true}).isVisible()),'Manual review stays on its own tab');
  await openTab(page,'Advanced');
  await page.getByText('Advanced destination settings',{exact:true}).click();
  await page.getByLabel('Google Ads customer ID').fill('1234567890');
  await page.getByLabel('Qualified inquiry action ID').fill('123');
  await page.getByLabel('Booking action ID').fill('456');
  await page.getByRole('button',{name:'Save destination',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Destination saved.'}).waitFor();
  await page.getByLabel('Conversion event',{exact:true}).selectOption('booking:ads-browser-booking');
  await page.getByText('Advertising user-data consent is not recorded as granted',{exact:true}).waitFor();
  assert(await page.getByRole('button',{name:'Stage reviewed conversion',exact:true}).isDisabled());
  await page.getByText('Record advertising consent evidence',{exact:true}).click();
  await page.getByLabel('Advertising user-data consent',{exact:true}).selectOption('granted');
  await page.getByLabel('Advertising personalization consent',{exact:true}).selectOption('denied');
  // Chromium normalizes zero seconds away; use its canonical minute precision.
  await page.getByLabel('Decision effective at (UTC)').fill(new Date(Date.now()-3600000).toISOString().slice(0,16));
  await page.getByLabel('Consent evidence reference').fill('Test consent receipt 123');
  await page.getByRole('button',{name:'Save consent evidence',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Consent evidence recorded.'}).waitFor();
  await page.getByRole('button',{name:'Stage reviewed conversion',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Conversion staged locally.'}).waitFor();
  let preview=(await api('/conversions')).body;
  let job=preview.jobs.find(j=>j.entity_key==='ads-browser-booking');
  assert.equal(job.snapshot.amount_minor,12345);assert.equal(preview.connected,false);
  assert.equal(preview.delivery_settings.enabled,false);
  const controls=page.getByRole('region',{name:'Google delivery controls',exact:true});
  await controls.getByRole('button',{name:'Enable reviewed delivery',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Delivery enabled.'}).waitFor();
  assert.equal((await api('/conversions')).body.delivery_settings.enabled,true);
  await controls.getByRole('button',{name:'Pause Google delivery',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Pending sends paused.'}).waitFor();
  assert.equal((await api('/conversions')).body.delivery_settings.enabled,false);
  await page.getByRole('button',{name:'Validate event with Google',exact:true}).click();
  await page.getByRole('alert').waitFor(); // No provider configured in this fixture.
  assert.deepEqual((await api('/conversions')).body.deliveries,[]);
  const originalId=job.id,originalTransaction=job.transaction_id;
  const c=preview.candidates.find(c=>c.entity_key==='ads-browser-booking');
  const retry=await api('/conversions/stage',{kind:c.kind,entity_key:c.entity_key,fingerprint:c.fingerprint,version:0});
  assert.equal(retry.status,200);assert.equal(retry.body.id,originalId);
  // A real booking revision invalidates the preparation, preserving its snapshot.
  assert.equal((await api(`/inquiries/${inquiry.id}/outcomes`,{...booking,amount_minor:67890,revision:2},'PUT')).status,200);
  await page.getByRole('button',{name:'Refresh conversions',exact:true}).click();
  const card=page.getByRole('article',{name:'Prepared booking ads-browser-booking',exact:true});
  await card.getByText('needs review',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Stage reviewed conversion',exact:true}).click();
  await card.getByText('awaiting connection',{exact:true}).waitFor();
  job=(await api('/conversions')).body.jobs.find(j=>j.entity_key==='ads-browser-booking');
  assert.equal(job.id,originalId);assert.equal(job.transaction_id,originalTransaction);assert.equal(job.snapshot.amount_minor,67890);
  await page.getByText('Review the prepared data',{exact:true}).click();
  await page.screenshot({path:path.join(artifactDir,'conversions-desktop.png'),fullPage:true});
  await page.setViewportSize({width:390,height:844});
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Conversions must fit mobile');
  await page.screenshot({path:path.join(artifactDir,'conversions-mobile.png'),fullPage:true});
  await page.setViewportSize({width:1440,height:1000});
  await card.getByRole('button',{name:'Cancel preparation',exact:true}).click();
  await card.getByText('canceled',{exact:true}).waitFor();
  const stale=await api(`/conversions/${job.id}/cancel`,{version:job.version});assert.equal(stale.status,409);
  assert.equal((await api('/conversions')).body.jobs.filter(j=>j.entity_key==='ads-browser-booking').length,1);
  await verifyPipeline({page,api,lead,artifactDir});
}
// Automatic conversions: actions, consent attestation, auto-send and event status.
async function verifyPipeline({page,api,lead,artifactDir}) {
  await openTab(page,'Actions');
  const section=page.getByRole('region',{name:'Conversion actions',exact:true});
  await section.getByRole('button',{name:'Add action',exact:true}).click();
  const form=section.getByRole('form',{name:'New action',exact:true});
  await form.getByLabel('Action key',{exact:true}).fill('order');
  await form.getByLabel('Action name',{exact:true}).fill('Order');
  await form.getByLabel('Counting',{exact:true}).selectOption('every');
  await form.getByLabel('Default value (USD, optional)',{exact:true}).fill('20.5');
  await form.getByLabel('Google conversion action ID',{exact:true}).fill('456');
  await form.getByRole('button',{name:'Create action',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Action created.'}).waitFor();
  const card=section.getByRole('article',{name:'Action order',exact:true});
  await card.getByText('Default $20.50',{exact:false}).waitFor();
  await card.getByRole('button',{name:'Edit action',exact:true}).click();
  const edit=card.getByRole('form',{name:'Edit action order',exact:true});
  assert(await edit.getByLabel('Counting',{exact:true}).isDisabled(),'Counting is fixed after creation');
  await edit.getByLabel('Action name',{exact:true}).fill('Completed order');
  await edit.getByRole('button',{name:'Save action',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Action saved.'}).waitFor();
  let overview=(await api('/conversions/pipeline')).body;
  assert.deepEqual(overview.actions.map(a=>[a.key,a.name,a.default_value_minor,a.google_action_id,a.version]),[['order','Completed order',2050,'456',2]]);
  await openTab(page,'Overview');
  const consent=page.getByRole('region',{name:'Advertising consent',exact:true});
  const auto=page.getByRole('region',{name:'Automatic sending',exact:true});
  assert.match(await page.getByRole('article',{name:'Google Ads status',exact:true}).getByLabel('Setup checklist').innerText(),/Turn on automatic sending: the owner in Conversions/);
  await consent.getByLabel('Not required: this workspace does not advertise in the EEA, UK or Switzerland').check();
  const save=consent.getByRole('button',{name:'Save consent policy',exact:true});
  assert(await save.isDisabled(),'The attestation must be confirmed first');
  await consent.getByLabel(/I confirm this workspace does not advertise/).check();
  await save.click();
  await page.getByRole('status').filter({hasText:'Consent policy saved.'}).waitFor();
  await consent.getByText(/^Attested by workspace operator/).waitFor();
  await auto.getByRole('button',{name:'Turn on automatic sending',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Automatic sending turned on.'}).waitFor();
  overview=(await api('/conversions/pipeline')).body;
  assert.equal(overview.policy.auto_send,true);assert.equal(overview.policy.consent_policy,'not_required_outside_eea');
  // Google delivery is paused in this fixture, so the event is recorded and never sent.
  const posted=await api('/v1/conversion-events',{action:'order',external_id:'browser-order-1',lead_id:lead.id,value_minor:41250});
  assert.equal(posted.status,200,JSON.stringify(posted.body));
  assert.equal(posted.body.delivery.google,'blocked');
  await openTab(page,'Events');
  await page.getByRole('button',{name:'Refresh events',exact:true}).click();
  const event=page.getByRole('article',{name:'Event order browser-order-1',exact:true});
  await event.getByText(/Recorded while automatic sending was off/).waitFor();
  await event.getByText('$412.50',{exact:false}).waitFor();
  await page.screenshot({path:path.join(artifactDir,'conversions-pipeline.png'),fullPage:true});
  await openTab(page,'Overview');
  await page.screenshot({path:path.join(artifactDir,'conversions-overview.png'),fullPage:true});
  await page.setViewportSize({width:390,height:844});
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Conversions tabs fit a phone screen');
  await page.screenshot({path:path.join(artifactDir,'conversions-overview-mobile.png'),fullPage:true});
  await openTab(page,'Actions');
  await page.screenshot({path:path.join(artifactDir,'conversions-actions-mobile.png'),fullPage:true});
  await page.setViewportSize({width:1440,height:1000});
  await page.screenshot({path:path.join(artifactDir,'conversions-actions.png'),fullPage:true});
  await openTab(page,'Microsoft Ads');
  await page.screenshot({path:path.join(artifactDir,'conversions-microsoft.png'),fullPage:true});
  await openTab(page,'Overview');
  await auto.getByRole('button',{name:'Turn off automatic sending',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Automatic sending turned off.'}).waitFor();
  await verifyMicrosoftPicker({page,artifactDir});
  await verifyMetaTab({page,artifactDir});
}
// Meta tab against stubbed endpoints; nothing reaches Meta. The token must leave the page only
// in the save request, and never come back.
async function verifyMetaTab({page,artifactDir}) {
  const TOKEN='EAAtest'+'x'.repeat(40);
  const status={revision:0,connected:false,dataset_id:null,dataset_name:null,token_fingerprint:null,token_saved_at:null,match:'meta_clicks',ldu:'off',settings_version:0,validation:null,checked_at:null,check_current:false};
  const sent=[];
  const handler=route=>{
    const r=route.request();const u=new URL(r.url()).pathname;const b=r.method()==='GET'?null:r.postDataJSON();
    if(u.endsWith('/connections/meta')){
      if(r.method()==='PUT'){sent.push(['connect',b]);Object.assign(status,{revision:1,connected:true,dataset_id:b.dataset_id,dataset_name:'Browser Pixel',token_fingerprint:'abc123def456',token_saved_at:new Date().toISOString()});return route.fulfill({json:{revision:1,dataset:{id:b.dataset_id,name:'Browser Pixel'},token_fingerprint:'abc123def456'}});}
      return route.fulfill({json:status});
    }
    if(u.endsWith('/meta-check')){sent.push(['check',b]);const report={valid:true,dataset:{id:status.dataset_id},test_event:{sent:true,trace_id:'trace-1',test_event_code:b.test_event_code},message:'Meta accepted a test Lead event.'};Object.assign(status,{validation:report,checked_at:new Date().toISOString(),check_current:true});return route.fulfill({json:report});}
    if(u.endsWith('/meta-settings')){sent.push(['settings',b]);Object.assign(status,{match:b.match,ldu:b.ldu,settings_version:1});return route.fulfill({json:{settings_version:1}});}
    sent.push(['event',b]);return route.fulfill({json:{}});
  };
  await page.route('**/api/connections/meta',handler);await page.route('**/api/conversion-setup/meta-*',handler);await page.route('**/api/conversion-actions/*/meta-event',handler);
  // This stage of the suite runs without an encryption key, which the real checklist reports.
  const pipeline=async route=>{const response=await route.fetch();const json=await response.json();if(json.setup?.meta)json.setup.meta.configured=true;return route.fulfill({response,json});};
  await page.route('**/api/conversions/pipeline',pipeline);
  try {
    await openTab(page,'Meta Ads');
    await page.getByRole('button',{name:'Refresh Meta connection',exact:true}).click();
    const token=page.getByLabel('Conversions API access token',{exact:true});
    await token.waitFor();
    assert.equal(await token.getAttribute('type'),'password');
    await page.getByText(/admin or developer of the business portfolio/).waitFor();
    await page.screenshot({path:path.join(artifactDir,'conversions-meta-connect.png'),fullPage:true});
    await page.getByLabel('Dataset (Pixel) ID',{exact:true}).fill('6870255032989723');
    await token.fill(TOKEN);
    await page.getByRole('button',{name:'Connect Meta',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Meta connected to dataset'}).waitFor();
    assert.deepEqual(sent.shift(),['connect',{revision:0,dataset_id:'6870255032989723',access_token:TOKEN}]);
    assert.equal(await token.count(),0,'The token field closes after saving');
    assert(!(await page.content()).includes(TOKEN),'The token is gone from the page');
    await page.getByText('abc123def456').waitFor();
    const row=page.getByRole('article',{name:'Meta event for order',exact:true});
    const picker=row.getByLabel('Meta event',{exact:true});
    await picker.selectOption('__custom__');
    const custom=row.getByLabel('Custom event name',{exact:true});
    await custom.fill('9bad');
    assert(await row.getByRole('button',{name:'Use this event',exact:true}).isDisabled(),'Invalid custom names cannot be saved');
    await picker.selectOption('Lead');
    await row.getByRole('button',{name:'Use this event',exact:true}).click();
    await page.getByRole('status').filter({hasText:'is sent to Meta as Lead'}).waitFor();
    const [kind,event]=sent.shift();assert.equal(kind,'event');assert.equal(event.event_name,'Lead');assert(Number.isInteger(event.version));
    await page.getByLabel('Test event code (optional)',{exact:true}).fill('TEST123');
    await page.getByRole('button',{name:'Send test event',exact:true}).click();
    await page.getByText('Meta accepted the test event',{exact:true}).waitFor();
    assert.deepEqual(sent.shift(),['check',{revision:1,test_event_code:'TEST123'}]);
    assert.equal(await page.getByRole('button',{name:'Save sharing settings',exact:true}).count(),0,'Sharing settings save only after a change');
    await page.getByLabel(/^Every eligible lead/).check();
    await page.getByRole('button',{name:'Save sharing settings',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Every eligible lead is now sent'}).waitFor();
    assert.deepEqual(sent.shift(),['settings',{settings_version:0,match:'all_leads',ldu:'off'}]);
    await page.screenshot({path:path.join(artifactDir,'conversions-meta.png'),fullPage:true});
    await page.setViewportSize({width:390,height:844});
    assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Meta tab fits a phone screen');
    await page.screenshot({path:path.join(artifactDir,'conversions-meta-mobile.png'),fullPage:true});
    await page.setViewportSize({width:1440,height:1000});
  } finally {
    await page.unroute('**/api/connections/meta',handler);await page.unroute('**/api/conversion-setup/meta-*',handler);await page.unroute('**/api/conversion-actions/*/meta-event',handler);await page.unroute('**/api/conversions/pipeline',pipeline);
  }
}
// Review actions: pause an unmapped goal or exclude one from bidding; a goal VisiConvert
// sends to offers no pause. Stubbed; nothing reaches Microsoft.
async function verifyMicrosoftReview(page) {
  const goals=[{id:'1',name:'Lead (Old CRM)',type:'OfflineConversion',status:'Active',exclude_from_bidding:false,used_by:[]},
    {id:'3',name:'Website lead',type:'Event',status:'Active',exclude_from_bidding:false,used_by:[]},
    {id:'4',name:'VisiConvert - lead',type:'OfflineConversion',status:'Paused',exclude_from_bidding:false,used_by:['lead']}];
  const review=()=>({summary:'3 goal(s) need a look.',goals,warnings:goals.map(g=>({id:g.id,name:g.name,flags:['other_offline_goal']})),flags:{other_offline_goal:'Another uploader may send the same leads.'}});
  const sent=[];
  const handler=route=>{const r=route.request();if(r.method()==='PUT'){const b=r.postDataJSON();sent.push(b);Object.assign(goals.find(g=>g.id===b.goal_id),b.status?{status:b.status}:{},b.exclude_from_bidding!==undefined?{exclude_from_bidding:b.exclude_from_bidding}:{});return route.fulfill({json:{changed:true}});}return route.fulfill({json:review()});};
  await page.route('**/api/conversion-setup/microsoft-{review,goal}',handler);
  try {
    await page.getByRole('button',{name:'Review other goals',exact:true}).click();
    const box=page.getByLabel('Other Microsoft goals',{exact:true});
    const item=name=>box.getByRole('listitem').filter({hasText:name});
    await item('Lead (Old CRM)').getByRole('button',{name:'Pause goal',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Paused “Lead (Old CRM)”'}).waitFor();
    assert.deepEqual(sent.shift(),{goal_id:'1',status:'Paused'});
    assert.equal(await item('Lead (Old CRM)').getByRole('button',{name:'Pause goal',exact:true}).count(),0);
    await item('Website lead').getByRole('button',{name:'Exclude from bidding',exact:true}).click();
    await page.getByRole('status').filter({hasText:'no longer counts in Conversions'}).waitFor();
    assert.deepEqual(sent.shift(),{goal_id:'3',exclude_from_bidding:true});
    assert.equal(await item('Website lead').getByRole('button').count(),1);
    assert.equal(await item('VisiConvert - lead').getByRole('button',{name:'Pause goal',exact:true}).count(),0);
    assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  } finally {
    await page.unroute('**/api/conversion-setup/microsoft-{review,goal}',handler);
  }
}
// Microsoft goal picker against a stubbed connected account; nothing is submitted.
async function verifyMicrosoftPicker({page,artifactDir}) {
  const status={configured:true,connected:true,reconnect_required:false,revision:1,redirect_uri:'',customer_id:'254773925',account_id:'189241031',account_name:'Browser Ads',destination_version:1,validation:null,checked_at:null};
  const goals=[{id:'1',name:'Lead (Old CRM)',status:'Active',kind:'OfflineConversion',category:'SubmitLeadForm',count_type:'All',window_minutes:43200,eligible:true},{id:'2',name:'Paused purchase',status:'Paused',kind:'OfflineConversion',category:'Purchase',count_type:'All',window_minutes:43200,eligible:false}];
  const handler=route=>{const u=new URL(route.request().url());const body=u.pathname.endsWith('/goals')?{goals}:u.pathname.endsWith('/accounts')?{accounts:[{id:'189241031',name:'Browser Ads',number:'X1',customer_id:'254773925',status:'Active',currency:'USD'}],has_more:false}:status;return route.fulfill({json:body});};
  await page.route('**/api/connections/microsoft**',handler);
  try {
    await openTab(page,'Microsoft Ads');
    await page.getByRole('button',{name:'Refresh Microsoft connection',exact:true}).click();
    const row=page.getByRole('article',{name:'Microsoft goal for order',exact:true});
    const picker=row.getByLabel('Microsoft goal',{exact:true});
    await picker.waitFor();
    const options=await picker.locator('option').allInnerTexts();
    assert.deepEqual(options,['Don’t send to Microsoft','Create a new VisiConvert goal','Lead (Old CRM)','Paused purchase · Paused']);
    assert(await picker.locator('option[value="Paused purchase"]').isDisabled());
    assert.equal(await picker.evaluate(e=>getComputedStyle(e).textTransform),'none','Goal names show exactly');
    await picker.selectOption('Lead (Old CRM)');
    await row.getByRole('button',{name:'Use this goal',exact:true}).waitFor();
    await picker.selectOption('__create__');
    await row.getByRole('button',{name:'Create goal',exact:true}).waitFor();
    await row.getByText(/Microsoft accepts uploads about two hours later/).waitFor();
    await page.screenshot({path:path.join(artifactDir,'conversions-microsoft-goal.png'),fullPage:true});
    await page.setViewportSize({width:390,height:844});
    assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Goal picker fits a phone screen');
    await page.screenshot({path:path.join(artifactDir,'conversions-microsoft-goal-mobile.png'),fullPage:true});
    await page.setViewportSize({width:1440,height:1000});
    await row.getByRole('button',{name:'Cancel',exact:true}).click();
    assert.equal(await picker.inputValue(),'');
    assert.equal(await row.getByRole('button',{name:'Create goal',exact:true}).count(),0);
    await verifyMicrosoftReview(page);
  } finally {
    await page.unroute('**/api/connections/microsoft**',handler);
  }
}
