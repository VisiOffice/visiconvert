import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
export async function verifyApiKeys({page,api,origin,artifactDir,root}) {
  const request=async(key,route,body)=>fetch(`${origin}/api/v1${route}`,{method:body?'POST':'GET',headers:{Authorization:`Bearer ${key}`,...(body?{'Content-Type':'application/json'}:{})},body:body?JSON.stringify(body):undefined});
  await page.getByRole('button',{name:'API keys',exact:true}).click();
  await page.getByLabel('Key name',{exact:true}).fill('Browser reader');
  await page.getByRole('button',{name:'Create API key',exact:true}).click();
  const secret=await page.getByLabel('New API key',{exact:true}).inputValue();
  assert(secret.startsWith('vc_key_'));
  const listed=(await api('/api-keys')).body;
  assert(!JSON.stringify(listed).includes(secret),'Metadata must never disclose the secret');
  assert.equal((await request(secret,'/workspace')).status,200);
  assert.equal((await request(secret,'/leads',{external_id:'must-not-create'})).status,403);
  const env={...process.env,VISICONVERT_URL:origin,VISICONVERT_API_KEY:secret,VISICONVERT_CONFIG:path.join(artifactDir,'missing-cli-config')};
  const cli=(args,input)=>spawnSync(path.join(root,'target/debug/visiconvert'),args,{env,input,encoding:'utf8',timeout:15000});
  let status=cli(['status','--json']);assert.equal(status.status,0,status.stderr);assert.equal(JSON.parse(status.stdout).healthy,true);
  const mcp=cli(['mcp','--allow-changes'],[
    {jsonrpc:'2.0',id:1,method:'initialize',params:{protocolVersion:'2025-06-18',capabilities:{},clientInfo:{name:'verification',version:'1'}}},
    {jsonrpc:'2.0',id:2,method:'tools/call',params:{name:'get_status',arguments:{}}},
    {jsonrpc:'2.0',id:3,method:'tools/call',params:{name:'add_lead',arguments:{name:'Denied',contact:'denied@example.com',source:'Test'}}}
  ].map(v=>JSON.stringify(v)).join('\n')+'\n');
  assert.equal(mcp.status,0,mcp.stderr);const replies=mcp.stdout.trim().split('\n').map(JSON.parse);assert(!replies.find(r=>r.id===2).result.isError);assert.equal(replies.find(r=>r.id===3).result.isError,true);
  await page.getByRole('button',{name:'I saved the key',exact:true}).click();
  await page.getByRole('button',{name:'Refresh keys',exact:true}).click();
  const card=page.getByRole('article',{name:'API key Browser reader',exact:true});
  await card.getByRole('button',{name:'Rotate key',exact:true}).click();
  await page.getByRole('button',{name:'Confirm rotate',exact:true}).click();
  const replacement=await page.getByLabel('New API key',{exact:true}).inputValue();assert(replacement!==secret);
  assert.equal((await request(secret,'/workspace')).status,401);assert.equal((await request(replacement,'/workspace')).status,200);
  await page.getByRole('button',{name:'I saved the key',exact:true}).click();
  const active=page.getByRole('article',{name:'API key Browser reader',exact:true}).filter({has:page.getByText('Active',{exact:true})});
  await active.getByRole('button',{name:'Revoke key',exact:true}).click();await page.getByRole('button',{name:'Confirm revoke',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Key revoked.'}).waitFor();assert.equal((await request(replacement,'/workspace')).status,401);
  await page.getByLabel('Key name',{exact:true}).fill('Browser writer');
  await page.getByLabel('Create interactions and review inquiries',{exact:true}).check();
  await page.getByLabel('Record, revise, and cancel bookings',{exact:true}).check();
  await page.getByRole('button',{name:'Create API key',exact:true}).click();
  const writer=await page.getByLabel('New API key',{exact:true}).inputValue();
  env.VISICONVERT_API_KEY=writer;
  const created=cli(['leads','add','--name','CLI public customer','--contact','cli@example.com','--source','API verification','--external-id','browser-cli-public','--json']);assert.equal(created.status,0,created.stderr);
  const lead=JSON.parse(created.stdout).id;
  const qualified=cli(['leads','qualify',lead,'qualified','--json']);assert.equal(qualified.status,0,qualified.stderr);
  const booked=cli(['leads','outcome',lead,'--ref','PUBLIC-CLI-BOOKING','--booked','123.45','--json']);assert.equal(booked.status,0,booked.stderr);
  const list=cli(['leads','list','--source','API verification','--json']);assert.equal(list.status,0,list.stderr);assert(JSON.parse(list.stdout).some(l=>l.id===lead));
  const booking=(await (await request(writer,'/bookings')).json()).data.find(b=>b.external_id==='PUBLIC-CLI-BOOKING');assert.equal(booking.amount_minor,12345);
  await page.getByRole('button',{name:'I saved the key',exact:true}).click();
  await page.screenshot({path:path.join(artifactDir,'api-keys-desktop.png'),fullPage:true});
  await page.setViewportSize({width:390,height:844});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'API keys must fit mobile');
  await page.screenshot({path:path.join(artifactDir,'api-keys-mobile.png'),fullPage:true});await page.setViewportSize({width:1440,height:1000});
  const writerCard=page.getByRole('article',{name:'API key Browser writer',exact:true});await writerCard.getByRole('button',{name:'Revoke key',exact:true}).click();await page.getByRole('button',{name:'Confirm revoke',exact:true}).click();await page.getByRole('status').filter({hasText:'Key revoked.'}).waitFor();
  status=cli(['status','--json']);assert.equal(status.status,1);assert(JSON.parse(status.stdout).error);
}
