import test from 'node:test';
import assert from 'node:assert/strict';
import {createRecovery} from '../server/openclaw-plugin/recovery.mjs';
const timeout={runId:'failed',success:false,error:'codex app-server request timed out'};
const ctx={sessionKey:'agent:main:d4c-chat'};
function fixture(options={}){const calls=[];const notices=[];const r=createRecovery({refresh:async home=>{calls.push(home);return 1;},notify:async n=>notices.push(n),...options});return {r,calls,notices};}
test('Codex timeout refreshes its managed connection once',async()=>{
 const {r,calls,notices}=fixture();await r.beforeRun(timeout,ctx);r.end(timeout,ctx);await r.settled();
 assert.deepEqual(calls,['main']);assert.equal(notices[0].status,'recovered');r.dispose();
});
test('active work defers recovery and a healthy turn cancels a queued refresh',async()=>{
 const {r,calls}=fixture();await r.beforeRun({runId:'other'},ctx);await r.beforeRun(timeout,ctx);
 r.end(timeout,ctx);await r.settled();assert.equal(calls.length,0);
 r.end({runId:'other',success:true},ctx);await r.settled();assert.equal(calls.length,0);r.dispose();
});
test('other agent active work also protects the shared Gateway',async()=>{
 const {r,calls}=fixture();const other={sessionKey:'agent:other:chat'};
 await r.beforeRun({runId:'other'},other);r.end(timeout,ctx);await r.settled();assert.equal(calls.length,0);
 r.end({runId:'other',success:true},other);await r.settled();assert.equal(calls.length,1);r.dispose();
});
test('new work waits for a refresh already in progress',async()=>{
 let release;const block=new Promise(resolve=>{release=resolve;});const {r}=fixture({refresh:()=>block});
 r.end(timeout,ctx);await new Promise(resolve=>setImmediate(resolve));let started=false;
 const next=r.beforeRun({runId:'next'},ctx).then(()=>{started=true;});await new Promise(resolve=>setImmediate(resolve));assert.equal(started,false);
 release(1);await next;assert.equal(started,true);r.dispose();
});
test('model errors never restart a connection',async()=>{
 const {r,calls}=fixture();r.end({...timeout,error:'This model was not found'},ctx);await r.settled();assert.equal(calls.length,0);r.dispose();
});
test('recovery failures consume a bounded hourly budget',async()=>{
 let time=0;const {r,calls,notices}=fixture({now:()=>time,cooldownMs:100,maxPerHour:2,refresh:async()=>{calls.push('try');throw new Error('unavailable');}});
 r.end(timeout,ctx);await r.settled();time=200;r.end({...timeout,runId:'fail-2'},ctx);await r.settled();time=400;r.end({...timeout,runId:'fail-3'},ctx);await r.settled();
 assert.equal(calls.length,2);assert.equal(notices.at(-1).status,'limited');r.dispose();
});

import {refreshManagedConnection} from '../server/openclaw-plugin/recovery.mjs';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
test('refresh terminates a real owned managed child but preserves a different home',async()=>{
 const root=await fs.mkdtemp(path.join(os.tmpdir(),'d4c-recovery-'));
 const script=path.join(root,'node_modules/@openai/codex/bin/codex.js');
 await fs.mkdir(path.dirname(script),{recursive:true});await fs.writeFile(script,'setInterval(()=>{},1000)');
 const start=home=>spawn(process.execPath,[script,'app-server'],{env:{...process.env,CODEX_HOME:home},stdio:'ignore'});
 const owned=start(path.join(root,'agents/main/agent/codex-home')),other=start(path.join(root,'manual-home'));
 try{await Promise.all([once(owned,'spawn'),once(other,'spawn')]);const exited=once(owned,'exit');
  assert.equal(await refreshManagedConnection('main',{state:root}),1);await exited;
  assert.equal(other.exitCode,null);assert.equal(other.signalCode,null);
  assert.equal(await refreshManagedConnection('main',{state:root}),0);
 }finally{owned.kill();other.kill();await fs.rm(root,{recursive:true,force:true});}
});
test('cooldown defers repeated timeouts and disposal cancels the queued refresh',async()=>{
 const {r,calls}=fixture({cooldownMs:40});r.end(timeout,ctx);await r.settled();
 r.end({...timeout,runId:'second'},ctx);await r.settled();assert.equal(calls.length,1);
 r.dispose();await new Promise(resolve=>setTimeout(resolve,60));assert.equal(calls.length,1);
});

test('OpenClaw context run IDs protect concurrent work',async()=>{
 const {r,calls}=fixture();const other={sessionKey:'agent:other:chat',runId:'context-run'};
 await r.beforeRun({},other);r.end(timeout,ctx);await r.settled();assert.equal(calls.length,0);
 r.end({success:true},other);await r.settled();assert.equal(calls.length,1);r.dispose();
});
test('native packaged executable is recognized without matching manual clients',async()=>{
 const root=await fs.mkdtemp(path.join(os.tmpdir(),'d4c-native-'));
 const binary=path.join(root,'node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex');
 await fs.mkdir(path.dirname(binary),{recursive:true});await fs.copyFile(process.execPath,binary);await fs.chmod(binary,0o755);
 const script=path.join(root,'app-server');await fs.writeFile(script,'setInterval(()=>{},1000)');
 const child=spawn(binary,[script,'app-server'],{env:{...process.env,CODEX_HOME:path.join(root,'agents/main/agent/codex-home')},stdio:'ignore'});
 try{await once(child,'spawn');const exited=once(child,'exit');assert.equal(await refreshManagedConnection('main',{state:root}),1);await exited;}
 finally{child.kill();await fs.rm(root,{recursive:true,force:true});}
});
