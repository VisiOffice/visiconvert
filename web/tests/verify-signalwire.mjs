import assert from 'node:assert/strict';

// UI contract test. The Rust integration suite tests the real DB/API with a simulated carrier.
export async function verifySignalWire({ page, api, apiKey, origin, artifactDir }) {
  assert.equal((await api('/connections/signalwire')).body.connected, false);
  const state = { connected:false, space_host:null, project_id:null, revision:0, last_webhook_at:null, setup_available:true, setup_message:null, can_disconnect:false, voice_url:origin+'/webhooks/signalwire/workspaces/fixture/voice', status_url:origin+'/webhooks/signalwire/workspaces/fixture/status' };
  let numberInput;
  const handler = async route => {
    const request=route.request(); const path=new URL(request.url()).pathname;
    let body=state;
    if(path.endsWith('/numbers')) body={numbers:[{sid:'fixture-number',phone_number:'+12025550311',friendly_name:'SignalWire test line'}],has_more:false};
    else if(request.method()==='PUT') {
      const data=request.postDataJSON();
      assert.equal(data.space_host,'fixture.signalwire.com'); assert.equal(data.project_id,'00000000-0000-4000-8000-000000000001');
      assert.equal(data.api_token,'fixture-api-token-not-a-credential'); assert.equal(data.signing_key,'fixture-signing-key-not-a-credential'); assert.equal(data.revision,0);
      Object.assign(state,{connected:true,space_host:data.space_host,project_id:data.project_id,revision:1,can_disconnect:true}); body={ok:true};
    } else if(request.method()==='DELETE') { assert.equal(request.postDataJSON().revision,1); state.connected=false; state.revision=0; state.can_disconnect=false; body={ok:true}; }
    await route.fulfill({contentType:'application/json',body:JSON.stringify(body)});
  };
  const register = async route => {
    if(route.request().method()!=='POST') return route.continue();
    numberInput=route.request().postDataJSON();
    await route.fulfill({contentType:'application/json',body:'{"ok":true}'});
  };
  await page.route('**/api/connections/signalwire**',handler);
  await page.route('**/api/numbers',register);
  try {
    // Re-login so the real refresh path loads both provider statuses.
    await page.reload();
    await page.getByLabel("Workspace API key").fill(apiKey);
    await page.getByRole("button", {name:"Open workspace",exact:true}).click();
    await page.getByRole('button',{name:'Connections',exact:true}).click();
    const panel=page.getByRole('region',{name:'SignalWire connection'});
    await panel.getByLabel('SignalWire Space hostname').fill('fixture.signalwire.com');
    await panel.getByLabel('SignalWire Project ID').fill('00000000-0000-4000-8000-000000000001');
    await panel.getByLabel('SignalWire API token').fill('fixture-api-token-not-a-credential');
    await panel.getByLabel('SignalWire webhook signing key').fill('fixture-signing-key-not-a-credential');
    await panel.getByRole('button',{name:'Verify and connect SignalWire',exact:true}).click();
    await panel.getByText('Waiting for the first signed callback.',{exact:false}).waitFor();
    assert.equal(await panel.getByLabel('SignalWire API token').inputValue(),'');
    assert.equal(await panel.getByLabel('SignalWire webhook signing key').inputValue(),'');
    await panel.getByRole('button',{name:'Check SignalWire numbers'}).click();
    await panel.getByText('+12025550311 · SignalWire test line',{exact:true}).waitFor();
    await page.screenshot({path:artifactDir+'/signalwire-desktop.png',fullPage:true});
    await page.setViewportSize({width:390,height:844});
    assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'SignalWire setup fits mobile');
    await page.screenshot({path:artifactDir+'/signalwire-mobile.png',fullPage:true});
    await page.setViewportSize({width:1440,height:1000});
    await page.getByRole('button',{name:'Tracking numbers',exact:true}).click();
    await page.getByRole('button',{name:'Add tracking number',exact:true}).click();
    const form=page.getByRole('dialog');
    await form.getByLabel('Phone provider',{exact:true}).selectOption('signalwire');
    for(const [label,value] of [['Label','SignalWire browser test'],['SignalWire tracking number','+12025550311'],['Business destination','+12025550398'],['Marketing source','Yard signs'],['Campaign','Pilot']]) await form.getByLabel(label,{exact:true}).fill(value);
    await form.getByRole('button',{name:'Register number'}).click();
    await form.waitFor({state:'hidden'});
    assert.equal(numberInput.provider,'signalwire'); assert.equal(numberInput.phone_number,'+12025550311');
    await page.getByRole('button',{name:'Connections',exact:true}).click();
    await panel.getByRole('button',{name:'Disconnect unused SignalWire project'}).click();
    await panel.getByText('Not connected',{exact:true}).waitFor();
  } finally {
    await page.unroute('**/api/connections/signalwire**',handler);
    await page.unroute('**/api/numbers',register);
    await page.reload();
    await page.getByLabel("Workspace API key").fill(apiKey);
    await page.getByRole("button", {name:"Open workspace",exact:true}).click();
  }
}
