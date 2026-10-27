#!/usr/bin/env python3
"""Generate the checked-in public v1 API contract; run --check in CI."""
import json
import sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def obj(properties, required=()): return {'type':'object','properties':properties,'required':list(required),'additionalProperties':False}
def ref(name): return {'$ref':f'#/components/schemas/{name}'}
def array(items): return {'type':'array','items':items}
def string(**kw): return {'type':'string',**kw}
def integer(**kw): return {'type':'integer',**kw}
uuid=string(format='uuid')
nullable_date={'type':['string','null'],'format':'date-time'}
qualification=string(enum=['unreviewed','qualified','unqualified','spam'])
schemas={
 'Error':obj({'error':string()},['error']),
 'Ok':obj({'ok':{'type':'boolean'}},['ok']),
 'Created':obj({'id':uuid},['id']),
 'NewInteraction':obj({'external_id':string(minLength=1,maxLength=150),'name':string(minLength=1,maxLength=100),'contact':string(minLength=1,maxLength=200),'source':string(minLength=1,maxLength=100),'campaign':string(maxLength=150),'notes':string(maxLength=4000)},['external_id','name','contact','source','campaign','notes']),
 'Review':obj({'version':integer(minimum=1),'qualification':qualification,'notes':string(maxLength=4000)},['version','qualification','notes']),
 'BookingInput':obj({'external_id':string(minLength=1,maxLength=150),'amount_minor':integer(minimum=0,maximum=100000000000),'currency':string(enum=['USD']),'status':string(enum=['booked','canceled']),'revision':integer(minimum=1)},['external_id','amount_minor','currency','status','revision']),
 'NumberInput':obj({k:string(minLength=1) for k in ['label','phone_number','destination','source','campaign']},['label','phone_number','destination','source','campaign']),
 'Interaction':obj({'id':uuid,'inquiry_id':uuid,'primary_lead_id':uuid,'name':string(),'contact':string(),'channel':string(enum=['call','form']),'source':string(),'campaign':string(),'qualification':qualification,'notes':string(),'version':integer(),'created_at':string(format='date-time'),'is_demo':{'type':'boolean'},'tracking_number':{'type':['string','null']},'destination':{'type':['string','null']},'call_status':{'type':['string','null']},'duration_seconds':{'type':['integer','null']},'booked_minor':integer()},['id','inquiry_id','name','source','qualification','version','created_at']),
 'Booking':obj({'external_id':string(),'lead_id':uuid,'inquiry_id':uuid,'amount_minor':integer(),'currency':string(enum=['USD']),'status':string(enum=['booked','canceled']),'revision':integer()},['external_id','amount_minor','currency','status','revision']),
 'Workspace':obj({'id':uuid,'name':string(),'currency':string(enum=['USD']),'hosted':{'type':'boolean'},'scopes':{'type':['array','null'],'items':string()}},['id','name','currency','hosted','scopes']),
 'SourceReport':obj({'source':string(),'leads':integer(),'qualified':integer(),'bookings':integer(),'booked_minor':integer()},['source','leads','qualified','bookings','booked_minor']),
 'Number':obj({'id':uuid,'label':string(),'phone_number':string(),'destination':string(),'source':string(),'campaign':string(),'is_demo':{'type':'boolean'}},['id','phone_number','destination','source','campaign']),
 'InquiryDetail':{'type':'object','description':'Inquiry, original interactions and latest 100 history entries; qualification/notes/version are shared by the inquiry.','required':['inquiry','interactions','history'],'properties':{'inquiry':{'type':'object'},'interactions':array({'type':'object'}),'history':array({'type':'object'})}},
 'TwilioStatus':{'type':'object','description':'Connection status, revision and callback URLs. Never includes provider secrets.','required':['connected','mode','voice_url','status_url'],'properties':{'connected':{'type':'boolean'},'mode':string(),'voice_url':string(),'status_url':string()}},
 'TwilioInventory':obj({'numbers':array(obj({'sid':string(),'phone_number':string(),'friendly_name':string()},['sid','phone_number','friendly_name'])),'has_more':{'type':'boolean'}},['numbers','has_more'])}
provider=string(enum=['twilio','signalwire'],default='twilio')
schemas['NumberInput']['properties']['provider']=provider
schemas['Number']['properties']['provider']=provider
schemas['SignalWireStatus']=obj({'connected':{'type':'boolean'},'space_host':{'type':['string','null']},'project_id':{'type':['string','null'],'format':'uuid'},'revision':integer(),'last_webhook_at':nullable_date,'setup_available':{'type':'boolean'},'setup_message':{'type':['string','null']},'can_disconnect':{'type':'boolean'},'voice_url':string(),'status_url':string()},['connected','revision','voice_url','status_url'])
for name in ['Interaction','Booking']:
 schemas[name+'Page']=obj({'data':array(ref(name)),'next_cursor':{'type':['string','null']}},['data','next_cursor'])
def content(schema):return {'application/json':{'schema':schema}}
paths={}
def route(path,method,summary,response,scope='read',body=None,paged=False,filters=False):
 op={'summary':summary,'operationId':method+'_'+path.strip('/').replace('/','_').replace('{','').replace('}','').replace('-','_'),'description':f'Requires `{scope}` permission.','security':[{'workspaceKey':[]}],'responses':{'200':{'description':'Success','content':content(response)}}}
 for code in [400,401,403,404,409,413,422,429,500,503]:
  op['responses'][str(code)]={'description':{400:'Validation failure',401:'Invalid, expired or revoked key',403:'Missing permission or disabled hosted access',404:'Not found in this workspace',409:'Conflicting version, revision or external ID',413:'Body exceeds 64 KiB',422:'Invalid JSON fields or types',429:'120 requests per key per minute exceeded',500:'Internal error',503:'Hosted access check unavailable'}[code],'content':content(ref('Error'))}
 op['responses']['429']['headers']={'Retry-After':{'schema':string(),'description':'Seconds to wait before retrying'}}
 params=[]
 if '{id}' in path: params.append({'in':'path','name':'id','required':True,'schema':uuid})
 if paged:params += [{'in':'query','name':'limit','schema':integer(minimum=1,maximum=100,default=50)},{'in':'query','name':'cursor','schema':string(),'description':'Opaque next_cursor from the previous page. Preserve the same filters.'}]
 if filters: params += [{'in':'query','name':'since','schema':string(format='date-time')},{'in':'query','name':'source','schema':string()},{'in':'query','name':'qualification','schema':qualification}]
 if params:op['parameters']=params
 if body:op['requestBody']={'required':True,'content':content(ref(body))}
 paths.setdefault(path,{})[method]=op
route('/workspace','get','Workspace and key permissions',ref('Workspace'))
for base in ['/interactions','/leads']:
 route(base,'get','List individual calls and form interactions',ref('InteractionPage'),paged=True,filters=True)
 route(base,'post','Create a form interaction; identical external_id and payload retries reuse the record',ref('Created'),'leads:write','NewInteraction')
 route(base+'/{id}','get','Get an interaction by full UUID without a list-size restriction',ref('Interaction'))
 route(base+'/{id}','patch','Update the entire inquiry shared by this interaction',ref('Ok'),'leads:write','Review')
 route(base+'/{id}/outcomes','get','Bookings attached to this original interaction',array(ref('Booking')))
 route(base+'/{id}/outcomes','put','Record or revise a booking; identical revision and payload is a safe retry',ref('Ok'),'bookings:write','BookingInput')
route('/inquiries','get','List active grouped inquiries',ref('InteractionPage'),paged=True,filters=True)
route('/inquiries/{id}','get','Inquiry details, interactions and history',ref('InquiryDetail'))
route('/inquiries/{id}','patch','Review a grouped inquiry',ref('Ok'),'leads:write','Review')
route('/inquiries/{id}/outcomes','get','Bookings across all inquiry interactions',array(ref('Booking')))
route('/inquiries/{id}/outcomes','put','Record a booking on an inquiry; existing bookings retain their original interaction',ref('Ok'),'bookings:write','BookingInput')
route('/bookings','get','List all workspace bookings, including canceled records',ref('BookingPage'),paged=True)
route('/reports/sources','get','All-time inquiry counts and booked revenue by primary source',array(ref('SourceReport')))
route('/numbers','get','List registered tracking numbers',array(ref('Number')))
route('/numbers','post','Register a number already owned in connected Twilio or SignalWire (provider defaults to twilio); webhook configuration remains manual',ref('Ok'),'numbers:write','NumberInput')
route('/connections/signalwire','get','Read SignalWire status and callback readiness; credential changes are not permitted',ref('SignalWireStatus'))
route('/connections/signalwire/numbers','get','Read first SignalWire inventory page; check has_more',ref('TwilioInventory'))
route('/connections/twilio','get','Read connection status; credential changes are not permitted',ref('TwilioStatus'))
route('/connections/twilio/numbers','get','Read first Twilio inventory page; check has_more',ref('TwilioInventory'))
spec={'openapi':'3.1.0','info':{'title':'VisiConvert public API','version':'1.0.0','description':'Workspace-scoped API for hosted and self-hosted integrations. Bearer keys are created in the workspace API keys screen. Keys cannot administer credentials, billing, or keys. Hosted requests fail closed when Rails access verification is unavailable. /leads aliases /interactions. Lists of inquiries, interactions and bookings use keyset pagination; numbers and source reports return arrays. No automatic request retries.'},'servers':[{'url':'/api/v1'}],'components':{'securitySchemes':{'workspaceKey':{'type':'http','scheme':'bearer','bearerFormat':'vc_key_...'}},'schemas':schemas},'paths':paths}
path=ROOT/'crates/convert-server/assets/openapi.json'
text=json.dumps(spec,indent=2)+'\n'
if '--check' in sys.argv:
 if path.read_text()!=text:raise SystemExit('OpenAPI is stale; run scripts/generate-openapi.py')
else:path.write_text(text)
