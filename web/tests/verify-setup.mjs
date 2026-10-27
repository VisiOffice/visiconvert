import assert from 'node:assert/strict';

// Runs against the real API and production frontend in the disposable verifier.
export async function verifySetup({page, api, apiKey, artifactDir}) {
  await page.getByRole('button',{name:'Set up this workspace',exact:true}).click();
  await page.getByRole('heading',{name:'Connect your agent',exact:true}).waitFor();
  await page.getByText('No websites yet.',{exact:false}).waitFor();
  const config = () => page.getByLabel('MCP configuration',{exact:true}).inputValue().then(JSON.parse);
  assert.deepEqual((await config()).mcpServers.visiconvert.args,['mcp']);
  await page.getByLabel('CLI config file path',{exact:true}).fill('/home/owner/.config/visiconvert/pilot.json');
  const mcp = (await config()).mcpServers.visiconvert;
  assert.equal(mcp.env.VISICONVERT_CONFIG,'/home/owner/.config/visiconvert/pilot.json');
  assert.equal(mcp.env.VISICONVERT_API_KEY,'');
  assert(!(await page.getByLabel('Agent setup prompt').inputValue()).includes(apiKey));
  await page.getByLabel('Allow setup changes through MCP').check();
  assert.deepEqual((await config()).mcpServers.visiconvert.args,['mcp','--allow-changes']);
  await page.getByRole('button',{name:'Prepare setup key',exact:true}).click();
  assert.equal(await page.getByLabel('Key name',{exact:true}).inputValue(),'Agent setup');
  assert.equal(await page.getByLabel('Create and update websites and DNI pools',{exact:true}).isChecked(),true);
  assert.equal(await page.getByLabel('Register existing Twilio or SignalWire numbers',{exact:true}).isChecked(),false);
  await page.getByRole('button',{name:'Create API key',exact:true}).click();
  const key = page.getByLabel('New API key',{exact:true});
  await key.waitFor();
  const secret = await key.inputValue();
  const metadata = (await api('/api-keys')).body.keys.find(k => k.name==='Agent setup');
  assert.deepEqual(metadata.scopes,['read','setup:write']);
  assert(new Date(metadata.expires_at)>new Date());
  await page.getByRole('button',{name:'I saved the key',exact:true}).click();
  // Create/update through the very same scoped public API used by MCP.
  const create = await api('/v1/sites',{name:'Onboarding test',origin:'https://onboarding.example',allowed_fields:['email'],enabled:true,version:0},'POST',secret);
  assert.equal(create.status,200);
  const id = create.body.id;
  assert.equal((await api(`/v1/sites/${id}/number-pool`,{version:0,fallback_phone:'+12025550979',enabled:false,number_ids:[],dedicated_numbers:false},'PUT',secret)).status,200);
  await page.getByRole('button',{name:'Setup',exact:true}).click();
  await page.getByRole('heading',{name:'Onboarding test',exact:true}).waitFor();
  await page.getByText('Tracker visit: Waiting for activity',{exact:true}).waitFor();
  await page.getByText('Form success reported by website: Waiting for activity',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Refresh setup status',exact:true}).click();
  await page.screenshot({path:artifactDir+'/setup-desktop.png',fullPage:true});
  await page.setViewportSize({width:390,height:844});
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Setup fits mobile');
  await page.screenshot({path:artifactDir+'/setup-mobile.png',fullPage:true});
  await page.setViewportSize({width:1280,height:720});
  await page.getByRole('button',{name:'Lock workspace',exact:true}).click();
  await page.getByLabel('Workspace API key',{exact:true}).waitFor();
  await page.getByLabel('Workspace API key',{exact:true}).fill(apiKey);
  await page.getByRole('button',{name:'Open workspace',exact:true}).click();
  await page.setViewportSize({width:1440,height:1000});
  // Keep later website tests deterministic; pause this fixture and revoke its key.
  assert.equal((await api(`/v1/sites/${id}`,{name:'Onboarding test',origin:'https://onboarding.example',allowed_fields:['email'],enabled:false,version:1},'PATCH',secret)).status,200);
  assert.equal((await api(`/api-keys/${metadata.id}/revoke`,{version:metadata.version})).status,200);
  await page.getByRole('button',{name:'Overview',exact:true}).click();
}
