// Run on the owned workspace server as root. Uses two ephemeral paired device identities.
// Creates a uniquely named fixture desktop; never selects a desktop by a user label.
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import net from 'node:net';
const host=process.env.D4C_HOSTNAME;
if(!host)throw new Error('D4C_HOSTNAME is required');
const url=process.env.D4C_CONTROL_URL || `wss://${host}:8443/control`;
const uid=execFileSync('id',['-u','d4c'],{encoding:'utf8'}).trim();
const fixture=`d4c-check-${crypto.randomUUID()}`;
const devices=[],links=[];
const delay=ms=>new Promise(r=>setTimeout(r,ms));
function admin(args){return execFileSync('runuser',['-u','d4c','--','/usr/local/bin/d4c-server',...args],{encoding:'utf8'});}
function identity(){const pair=crypto.generateKeyPairSync('ed25519');const raw=pair.publicKey.export({type:'spki',format:'der'}).subarray(-32);const id=crypto.createHash('sha256').update(raw).digest('hex');devices.push(id);return{...pair,raw,id};}
async function connect(key){
 return new Promise((resolve,reject)=>{
  const ws=new WebSocket(url);let revision=0,objects=new Map(),pending=new Map();let paired=false;
  const timer=setTimeout(()=>{ws.close();reject(new Error('Pairing timed out'));},15000);
  ws.onerror=()=>{if(paired)return;clearTimeout(timer);reject(new Error('Workspace connection failed'));};
  function snapshot(v){objects.clear();revision=v.revision;for(const[k,rows]of Object.entries(v))if(Array.isArray(rows))for(const r of rows)objects.set(`${k}:${r.id}`,r);}
  ws.onmessage=async({data})=>{try{const v=JSON.parse(String(data));
   if(v.type==='challenge'){assert.equal(v.hostname,host);const signature=crypto.sign(null,Buffer.from(`d4c-service-v1|${host}|${v.nonce}`),key.privateKey);ws.send(JSON.stringify({deviceId:key.id,publicKey:key.raw.toString('base64url'),signature:signature.toString('base64url')}));}
   if(v.type==='pairing_required'){paired=true;clearTimeout(timer);ws.close();admin(['--approve',key.id]);resolve(await connect(key));}
   if(v.type==='snapshot'){snapshot(v.payload);clearTimeout(timer);const link={ws,key,objects,rpc(method,params){return new Promise((resolve,reject)=>{const id=crypto.randomUUID();const timer=setTimeout(()=>{pending.delete(id);reject(new Error(`RPC timed out: ${method}`));},20000);pending.set(id,{resolve,reject,timer});ws.send(JSON.stringify({id,method,params}));});}};links.push(link);resolve(link);}
   if(v.type==='changes'){const p=v.payload;if(p.resync)snapshot(p.snapshot);else{revision=p.revision;for(const e of p.events)objects.set(`${e.kind}:${e.id}`,{...e.value,rev:e.seq});}}
   if(v.type==='response'){const p=pending.get(v.id);if(p){clearTimeout(p.timer);pending.delete(v.id);if(v.ok)p.resolve(v.payload);else p.reject(new Error(v.error));}}
  }catch(e){clearTimeout(timer);reject(e);}};
  ws.onclose=()=>{clearTimeout(timer);for(const p of pending.values()){clearTimeout(p.timer);p.reject(new Error('Workspace disconnected'));}if(!paired)pending.clear();};
 });
}
async function waitFor(test,description){for(let i=0;i<120;i++){if(await test())return;await delay(250);}throw new Error(`Timed out: ${description}`);}
function podman(args){return execFileSync('runuser',['-u','d4c','--','env',`XDG_RUNTIME_DIR=/run/user/${uid}`,`DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/${uid}/bus`,'sh','-c','cd "$HOME" && exec podman "$@"','d4c-check',...args],{encoding:'utf8'}).trim();}
function ipc(session,method,params){return new Promise((resolve,reject)=>{const s=net.createConnection('/run/d4c/manager.sock');let bytes='';s.on('error',reject);s.setTimeout(15000,()=>s.destroy(new Error('IPC timeout')));s.on('connect',()=>s.write(JSON.stringify({session,method,params})+'\n'));s.on('data',b=>{bytes+=b.toString();if(bytes.includes('\n')){s.end();resolve(JSON.parse(bytes.split('\n')[0]));}});});}
let desktop;
try{
 let a=await connect(identity()),b=await connect(identity());
 let draft=await a.rpc('workspace.put',{kind:'draft',id:fixture,expectedRevision:0,value:{text:'Server draft'}});
 await waitFor(()=>b.objects.get(`draft:${fixture}`)?.text==='Server draft','second device receives draft');
 assert.equal(b.objects.get(`draft:${fixture}`).rev,draft.rev);
 await assert.rejects(()=>b.rpc('workspace.put',{kind:'draft',id:fixture,expectedRevision:0,value:{text:'Stale'}}),/conflict/);
 console.log('PASS two-device draft sync and stale-write rejection');
 await a.rpc('workspace.put',{kind:'preference',id:fixture,expectedRevision:0,value:{session:fixture}});
 await a.rpc('notification.import',{id:fixture,session:fixture,title:'Verification',body:'Server result',unread:true,time:Date.now()});
 await waitFor(()=>b.objects.get(`notification:${fixture}`)?.unread===true,'second device receives notification');
 await b.rpc('notification.read',{id:fixture});
 await waitFor(()=>a.objects.get(`notification:${fixture}`)?.unread===false,'read state returns to first device');
 console.log('PASS shared notification and read state');
 desktop=await a.rpc('desktop.create',{name:fixture});
 console.log('Fixture desktop:',desktop.id);
 await waitFor(()=>a.objects.get(`desktop:${desktop.id}`)?.status==='stopped','desktop preparation');
 desktop=a.objects.get(`desktop:${desktop.id}`);
 await a.rpc('desktop.start',{id:desktop.id,expectedRevision:desktop.rev});
 await waitFor(()=>a.objects.get(`desktop:${desktop.id}`)?.status==='running','rootless desktop starts');
 const container=`d4c-${desktop.id}`;const sibling=process.argv[2];if(sibling){assert.match(sibling,/^[a-f0-9-]{36}$/);assert.notEqual(sibling,desktop.id);}
 podman(['exec',container,'sh','-c','printf D4C_PERSISTENT > /workspace/d4c-verification-marker']);
 if(sibling){podman(['exec',`d4c-${sibling}`,'test','!','-e','/workspace/d4c-verification-marker']);const mounts=JSON.parse(podman(['inspect','--format','{{json .Mounts}}',container]));assert(mounts.every(m=>!m.Source.includes(sibling)));console.log('PASS sibling workspace file and mount isolation');}
 const binding=await a.rpc('desktop.bind',{session:fixture,desktopId:desktop.id,expectedRevision:0});
 assert.equal(binding.desktopId,desktop.id);
 await a.rpc('desktop.claim',{id:desktop.id});
 await assert.rejects(()=>b.rpc('desktop.input',{id:desktop.id,type:'pointer',x:10,y:10,buttons:0}),/lease/);
 assert.equal((await ipc(fixture,'tool.move',{x:10,y:10})).ok,false);
 console.log('PASS server-enforced human takeover across devices and agent IPC');
 const pid=podman(['inspect','--format','{{.State.Pid}}',container]);
 for(const link of links)link.ws.close();await delay(500);
 assert.equal(podman(['inspect','--format','{{.State.Pid}}',container]),pid);
 assert.equal(podman(['exec',container,'cat','/workspace/d4c-verification-marker']),'D4C_PERSISTENT');
 execFileSync('systemctl',['restart','d4c-manager.service']);
 await delay(1500);a=await connect(a.key);
 assert.equal(a.objects.get(`desktop:${desktop.id}`).mode,'human');
 assert.equal(a.objects.get(`draft:${fixture}`).text,'Server draft');
 assert.equal(a.objects.get(`preference:${fixture}`).session,fixture);
 assert.equal(a.objects.get(`notification:${fixture}`).unread,false);
 assert.equal(podman(['inspect','--format','{{.State.Pid}}',container]),pid);
 assert.equal(podman(['exec',container,'cat','/workspace/d4c-verification-marker']),'D4C_PERSISTENT');
 console.log('PASS clients close and manager restart preserve desktop PID/files/draft/human pause');
 await a.rpc('desktop.return_agent',{id:desktop.id});
 assert.equal((await ipc(fixture,'tool.move',{x:20,y:20})).ok,true);
 const screenshot=await ipc(fixture,'tool.screenshot',{});assert.equal(screenshot.ok,true);assert.equal(screenshot.payload.content[0].type,'image');
 console.log('PASS agent resumes and captures real framebuffer');
 // Only the newly generated fixture desktop is deleted. Existing user desktops are untouched.
 desktop=(await a.rpc('desktop.list',{})).desktops.find(d=>d.id===desktop.id);
 await a.rpc('desktop.delete',{id:desktop.id,expectedRevision:desktop.rev,confirm:true});
 await waitFor(()=>a.objects.get(`desktop:${desktop.id}`)?.deleted===true,'fixture desktop removal');
 console.log('PASS fixture lifecycle cleanup');
}catch(error){console.error('FAIL',error.message);process.exitCode=1;}
finally{
 for(const link of links)link.ws.close();
 // Remove only the ephemeral test device approvals and draft/binding rows.
 const cleanup=`import sqlite3,json,sys\nc=sqlite3.connect('/var/lib/d4c/workspace.sqlite')\nfor device in json.loads(sys.argv[1]): c.execute('DELETE FROM devices WHERE id=?',(device,))\nfor kind in ['draft','binding','preference','notification']: c.execute('DELETE FROM objects WHERE kind=? AND id=?',(kind,sys.argv[2]))\nc.commit()`;
 execFileSync('python3',['-c',cleanup,JSON.stringify(devices),fixture]);
}
