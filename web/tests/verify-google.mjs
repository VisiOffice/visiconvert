import assert from 'node:assert/strict';

// UI contract coverage with a deterministic provider boundary. The Rust suite
// separately exercises the real API/database and a local OAuth/Ads server.
export async function verifyGoogle({ page, api, origin, artifactDir }) {
  await page.getByRole('button', { name:'Conversions', exact:true }).click();
  await page.getByRole('heading', { name:'Connect Google Ads', exact:true }).waitFor();
  await page.getByText('Google connection is not configured on this installation.', { exact:false }).waitFor();
  assert.equal((await api('/connections/google')).body.configured,false);
  assert.equal((await api('/connections/google/start',{revision:0})).status,503);
  const status={configured:true,connected:false,reconnect_required:false,revision:0,redirect_uri:origin+'/auth/google/callback',validation:null,checked_at:null};
  let lastSettings;
  let logins = ["1234567890"];
  let discoveryFails = false;
  let creationFails = true;
  let creationRequests = 0;
  const handler=async route=>{
    const request=route.request();const url=new URL(request.url());const data=request.postDataJSON();
    let body=status;
    if(url.pathname.endsWith('/start')) body={authorization_url:'https://accounts.google.com/o/oauth2/v2/auth?test=visiconvert'};
    else if(url.pathname.endsWith('/accounts')) {
      if(discoveryFails) return route.fulfill({status:503,json:{error:'Account discovery temporarily unavailable'}});
      body={logins,accounts:url.searchParams.has('login')?[{id:'1234567890',name:'Browser test Ads',manager:false,status:'ENABLED'}]:[],has_more:false};
    }
    else if(url.pathname.endsWith('/actions/create')) {
      creationRequests++;
      assert.equal(data.revision,1);assert.equal(data.customer,'1234567890');assert.equal(data.login,'1234567890');
      assert.equal(data.qualified,true);assert.equal(data.booking,true);
      if(creationFails) return route.fulfill({status:502,json:{error:'Creation could not be confirmed. Retry to recover actions.'}});
      const current=(await api('/conversions')).body.settings;
      assert.equal(data.settings_version,current.version);
      assert.equal((await api('/conversions/settings',{...current,customer_id:data.customer,qualified_action_id:'123',booking_action_id:'456'},'PUT')).status,200);
      body={ok:true,qualified_action_id:'123',booking_action_id:'456'};
    }
    else if(url.pathname.endsWith('/actions')) body={account:{id:'1234567890',name:'Browser test Ads',conversion_owner:'1234567890'},actions:[{id:'123',name:'Qualified inquiries',eligible:true,kind:'UPLOAD_CLICKS',status:'ENABLED',owner:'1234567890',primary:false},{id:'456',name:'Booked revenue',eligible:true,kind:'UPLOAD_CLICKS',status:'ENABLED',owner:'1234567890',primary:true},{id:'999',name:'Website action',eligible:false,kind:'WEBPAGE',status:'ENABLED',owner:'1234567890',primary:false}],has_more:false};
    else if(url.pathname.endsWith('/validate')) {
      lastSettings=(await api('/conversions')).body.settings;
      assert.equal(data.settings_version,lastSettings.version);assert.equal(data.login,'1234567890');assert.equal(data.revision,1);
      status.validation={valid:true,message:'Account and import actions are valid. No uploads were performed.',settings_version:data.settings_version,login_customer_id:data.login};status.checked_at=new Date().toISOString();body=status.validation;
    }else if(request.method()==='DELETE'){status.connected=false;status.revision++;status.validation=null;body={ok:true};}
    await route.fulfill({contentType:'application/json',body:JSON.stringify(body)});
  };
  const authorization=route=>route.fulfill({contentType:'text/html',body:'<!doctype html><title>Test Google</title><h1>Test Google authorization</h1>'});
  await page.route('**/api/connections/google**',handler);
  await page.context().route('https://accounts.google.com/o/oauth2/v2/auth?test=visiconvert',authorization);
  try {
    await page.getByRole('button',{name:'Overview',exact:true}).click();
    await page.getByRole('button',{name:'Conversions',exact:true}).click();
    const popupPromise=page.waitForEvent('popup');
    await page.getByRole('button',{name:'Sign in with Google',exact:true}).click();
    const popup=await popupPromise;
    await popup.getByRole('heading',{name:'Test Google authorization'}).waitFor();
    status.connected=true;status.revision=1;
    await popup.goto(origin+'/auth/google/result?status=connected').catch(e=>{if(!popup.isClosed())throw e;});
    await page.getByText('Google sign-in complete',{exact:true}).waitFor();
    assert.equal((await api('/bootstrap')).status,200);
    await page.getByLabel('Advertising account',{exact:true}).selectOption('1234567890');
    assert.equal(creationRequests,0);
    await page.getByLabel('Qualified leads',{exact:true}).uncheck();
    await page.getByLabel('Bookings',{exact:true}).uncheck();
    assert(await page.getByRole('button',{name:'Create conversion actions',exact:true}).isDisabled());
    await page.getByLabel('Qualified leads',{exact:true}).check();
    await page.getByLabel('Bookings',{exact:true}).check();
    await page.getByRole('button',{name:'Create conversion actions',exact:true}).click();
    await page.getByRole('alert').filter({hasText:'Creation could not be confirmed'}).waitFor();
    creationFails=false;
    await page.getByRole('button',{name:'Create conversion actions',exact:true}).click();
    await page.getByText('Conversion actions created or reused and saved. Check the saved connection below. Delivery remains unchanged.',{exact:true}).waitFor();
    assert.equal(creationRequests,2);
    await page.setViewportSize({width:390,height:844});
    assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
    await page.locator('.google-connection').screenshot({path:artifactDir+'/google-setup-mobile.png'});
    await page.setViewportSize({width:1440,height:1000});
    await page.getByText('Use existing actions (advanced)',{exact:true}).click();
    await page.getByLabel('Qualification conversion action',{exact:true}).selectOption('123');
    await page.getByLabel('Booking conversion action',{exact:true}).selectOption('456');
    assert(await page.getByLabel('Booking conversion action',{exact:true}).locator('option[value="999"]').isDisabled());
    await page.getByRole('button',{name:'Save conversion actions',exact:true}).click();
    await page.getByText('Conversion actions saved. Check the saved connection below to finish setup. Delivery remains unchanged.',{exact:true}).waitFor();
    await page.getByRole('button',{name:'Check saved connection',exact:true}).click();
    await page.getByText('Destination checks passed',{exact:true}).waitFor();
    await page.screenshot({path:artifactDir+'/google-desktop.png',fullPage:true});
    await page.setViewportSize({width:390,height:844});
    assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
    await page.screenshot({path:artifactDir+'/google-mobile.png',fullPage:true});
    await page.setViewportSize({width:1440,height:1000});
    // Reopening setup discovers accounts and restores mappings without writing settings.
    const savedSettings=(await api('/conversions')).body.settings;
    const reopen=async()=>{await page.getByRole('button',{name:'Overview',exact:true}).click();await page.getByRole('button',{name:'Conversions',exact:true}).click();};
    const existing=async()=>{await page.getByText('Use existing actions (advanced)',{exact:true}).click();};
    await reopen();
    await existing();
    await page.getByLabel('Booking conversion action',{exact:true}).waitFor();
    assert.equal(await page.getByLabel('Advertising account',{exact:true}).inputValue(),'1234567890');
    assert.match(await page.getByLabel('Advertising account',{exact:true}).locator('option:checked').innerText(),/Browser test Ads/);
    assert.equal(await page.getByLabel('Booking conversion action',{exact:true}).inputValue(),'456');
    assert.deepEqual((await api('/conversions')).body.settings,savedSettings);
    // An API failure remains retryable from the connected state.
    discoveryFails=true;
    await reopen();
    await page.getByRole('alert').filter({hasText:'Account discovery temporarily unavailable'}).waitFor();
    discoveryFails=false;
    await page.getByRole('button',{name:'Refresh Google Ads accounts',exact:true}).click();
    await existing();
    await page.getByLabel('Booking conversion action',{exact:true}).waitFor();
    // Multiple access accounts require an explicit choice if no saved check identifies one.
    logins=['1234567890','9876543210'];status.validation=null;
    await reopen();
    await page.getByLabel('Access through a Google Ads account',{exact:true}).selectOption('1234567890');
    await existing();
    await page.getByLabel('Booking conversion action',{exact:true}).waitFor();
    assert.deepEqual((await api('/conversions')).body.settings,savedSettings);
    logins=[];
    await reopen();
    await page.getByText('No Google Ads accounts are accessible with this Google login.',{exact:false}).waitFor();
    logins=['1234567890'];
    await reopen();
    await existing();
    await page.getByLabel('Booking conversion action',{exact:true}).waitFor();
    await page.getByRole('button',{name:'Check saved connection',exact:true}).click();
    await page.getByText('Destination checks passed',{exact:true}).waitFor();
    await page.getByText('Advanced destination settings',{exact:true}).click();
    await page.getByLabel('Booking action ID',{exact:true}).fill('789');
    await page.getByRole('button',{name:'Save destination',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Destination saved.'}).waitFor();
    await page.getByText('Destination checks passed',{exact:true}).waitFor({state:'hidden'});
    await page.getByRole('button',{name:'Disconnect Google',exact:true}).click();
    await page.getByText('Google not connected',{exact:true}).waitFor();
  }finally {
    await page.unroute('**/api/connections/google**',handler);
    await page.context().unroute('https://accounts.google.com/o/oauth2/v2/auth?test=visiconvert',authorization);
  }
}
