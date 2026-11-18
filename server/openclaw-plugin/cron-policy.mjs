// Scheduled D4C work must never execute in the interactive conversation.
export const isD4cSession = session => typeof session === 'string' && /^agent:[^:]+:d4c-[^:]+$/.test(session);
export const schedulingContext = 'D4C scheduled tasks run in isolated sessions with delivery mode none. Preserve the originating owner/sessionKey for Tasks and notifications; never use current/main/session targets or announce results into the live chat. Results remain in task run history and owner-linked notifications.';
export function isolatedJob(job) {
 const isolated={...job,sessionTarget:'isolated',delivery:{...job.delivery,mode:'none'}};
 if(job.payload?.kind==='systemEvent'){
  const {text,kind,...rest}=job.payload;
  isolated.payload={...rest,kind:'agentTurn',message:text};
 }
 return isolated;
}
export function scheduledToolPolicy(event,ctx){
 if(!isD4cSession(ctx.sessionKey)||!['cron','automations'].includes(event.toolName))return;
 const params=event.params;
 if(!['add','update'].includes(params?.action))return;
 // OpenClaw's native automation tool accepts add/update definitions as `job`.
 if(!params.job||typeof params.job!=='object'||Array.isArray(params.job))return {block:true,blockReason:'D4C scheduling requires an explicit job object. Use an isolated agentTurn with delivery mode none and preserve its originating owner.'};
 return {params:{...params,job:isolatedJob(params.job)}};
}
