import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {isD4cSession} from './cron-policy.mjs';

// Only children launched by this Gateway, with this managed CODEX_HOME, qualify.
export async function refreshManagedConnection(agent,{state=process.env.OPENCLAW_STATE_DIR||path.join(os.homedir(),'.openclaw'),parent=process.pid,proc='/proc',signal=process.kill}={}) {
 if(!/^[a-zA-Z0-9_-]{1,80}$/.test(agent))throw new Error('Invalid agent');
 const home=path.join(state,'agents',agent,'agent','codex-home');
 const read=async pid=>{
  const [stat,cmd,env]=await Promise.all(['stat','cmdline','environ'].map(n=>fs.readFile(path.join(proc,String(pid),n),'utf8')));
  const fields=stat.slice(stat.lastIndexOf(')')+2).split(' ');
  return {parent:Number(fields[1]),start:fields[19],cmd:cmd.split('\0'),env:env.split('\0')};
 };
 const matches=p=>p.parent===parent&&p.cmd.some(x=>x.endsWith('/@openai/codex/bin/codex.js')||/\/@openai\/codex-(?:linux|darwin)-[^/]+\/vendor\/[^/]+\/bin\/codex$/.test(x))&&p.cmd.includes('app-server')&&p.env.includes(`CODEX_HOME=${home}`);
 let count=0;
 for(const entry of await fs.readdir(proc)) {
  if(!/^\d+$/.test(entry))continue;
  try {
   const pid=Number(entry),first=await read(pid);
   if(!matches(first))continue;
   const current=await read(pid);
   if(!matches(current)||first.start!==current.start)continue;
   signal(pid,'SIGTERM');
   let exited=false;
   for(let i=0;i<100;i++){
    await new Promise(resolve=>setTimeout(resolve,50));
    try{const after=await read(pid);if(after.start!==first.start){exited=true;break;}}
    catch(error){if(['ENOENT','ESRCH'].includes(error.code)){exited=true;break;}throw error;}
   }
   if(!exited)throw new Error('Managed Codex connection did not exit');
   count++;
  }catch(error){if(!['ENOENT','ESRCH','EACCES','EPERM'].includes(error.code))throw error;}
 }
 return count;
}

export function createRecovery({refresh=refreshManagedConnection,notify=async()=>{},now=Date.now,cooldownMs=120000,maxPerHour=3}={}) {
 if(!Number.isFinite(cooldownMs)||cooldownMs<1||!Number.isInteger(maxPerHour)||maxPerHour<1||maxPerHour>20)throw new Error('Invalid recovery limits');
 const active=new Map(),pending=new Map(),attempts=new Map();let flight=Promise.resolve(),busy=false,disposed=false,timer;
 const agentOf=ctx=>{const id=ctx?.agentId||/^agent:([^:]+):/.exec(ctx?.sessionKey||'')?.[1];return /^[a-zA-Z0-9_-]{1,80}$/.test(id||'')?id:null;};
 const report=async (job,status)=>{try{await notify({...job,status});}catch{/* Observer outage must not fail a user turn. */}};
 function drain(){
  if(disposed||busy||active.size||!pending.size)return;
  clearTimeout(timer);timer=undefined;busy=true;
  flight=(async()=>{
   for(const [agent,job] of pending){
    if(disposed||active.size)break;
    const time=now(),history=(attempts.get(agent)||[]).filter(t=>time-t<3600000);
    if(history.length>=maxPerHour){pending.delete(agent);await report(job,'limited');continue;}
    const delay=history.length?cooldownMs-(time-history.at(-1)):0;
    if(delay>0){timer=setTimeout(()=>{timer=undefined;drain();},delay);timer.unref?.();break;}
    pending.delete(agent);history.push(time);attempts.set(agent,history);
    try{const count=await refresh(agent);if(!disposed)await report(job,count?'recovered':'unavailable');}
    catch{if(!disposed)await report(job,'failed');}
   }
  })().finally(()=>{busy=false;});
 }
 return {
  async beforeRun(event,ctx){while(busy)await flight;if(disposed)return;const agent=agentOf(ctx);const run=event.runId??ctx?.runId;if(agent&&run)active.set(run,agent);},
  end(event,ctx){
   if(disposed)return;active.delete(event.runId??ctx?.runId);const agent=agentOf(ctx);
   if(agent&&event.success===true)pending.delete(agent);
   const error=String(event.error?.message??event.error??'');
   if(agent&&isD4cSession(ctx?.sessionKey)&&event.success===false&&/codex[\s\S]*app[- ]server[\s\S]*(?:timed? out|timeout)/i.test(error)){
    if(pending.size<128||pending.has(agent))pending.set(agent,{agent,session:ctx.sessionKey,runId:event.runId});
   }
   // Prune idle budget records, keeping long-lived gateways bounded.
   for(const [key,history] of attempts)if(!pending.has(key)&&history.every(t=>now()-t>=3600000))attempts.delete(key);
   drain();
  },
  dispose(){disposed=true;clearTimeout(timer);pending.clear();active.clear();},
  async settled(){await flight;},
 };
}
