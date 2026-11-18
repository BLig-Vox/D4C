// A bounded, read-only MCP handshake/listing. Secrets arrive on stdin/local files only.
import fs from 'node:fs';
import path from 'node:path';
import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';
const packageRoot=process.env.OPENCLAW_PACKAGE_ROOT || path.join(execFileSync('npm',['root','-g'],{encoding:'utf8',timeout:10000}).trim(),'openclaw');
const require = createRequire(path.join(packageRoot,'package.json'));
const {Client} = await import(require.resolve('@modelcontextprotocol/sdk/client/index.js'));
const {StreamableHTTPClientTransport} = await import(require.resolve('@modelcontextprotocol/sdk/client/streamableHttp.js'));
const {SSEClientTransport} = await import(require.resolve('@modelcontextprotocol/sdk/client/sse.js'));
const job = JSON.parse(fs.readFileSync(0,'utf8'));
const root = process.env.OPENCLAW_STATE_DIR ?? path.join(process.env.HOME,'.openclaw');
const env = {...require('dotenv').parse(fs.existsSync(path.join(root,'.env')) ? fs.readFileSync(path.join(root,'.env')) : ''),...process.env};
let client, transport;
const timeout = setTimeout(()=>{console.log(JSON.stringify({ok:false,reason:'MCP connection timed out; previous settings kept'}));process.exit(1)},18000);
try {
  // OpenClaw itself serializes JSON here; unsupported custom include layouts fail
  // before publication rather than guessing at existing authentication settings.
  const config = require('json5').parse(fs.readFileSync(process.env.OPENCLAW_CONFIG_PATH ?? path.join(root,'openclaw.json'),'utf8'));
  const old = config.mcp?.servers?.[job.name] ?? {};
  if (config.$include || config.mcp?.$include || config.mcp?.servers?.$include || old.$include)
    throw new Error('unsupported-layout');
  if (old.url && old.url !== job.url) throw new Error('name-collision');
  if (old.auth === 'oauth') throw new Error('oauth');
  const headers = Object.fromEntries(Object.entries(old.headers??{}).map(([k,v])=>[k,String(v).replace(/\$\{([A-Z0-9_]+)\}/g,(_,key)=>{if(!env[key])throw new Error('missing-secret');return env[key]})]));
  const previousAuth=Object.entries(headers).filter(([k])=>k.toLowerCase()==='authorization');
  let authUnchanged=false;
  if (job.bearer_file) {
    for(const key of Object.keys(headers)) if(key.toLowerCase()==='authorization') delete headers[key];
    headers.Authorization = 'Bearer '+fs.readFileSync(job.bearer_file,'utf8').trim();
    authUnchanged=previousAuth.length===1 && previousAuth[0][1]===headers.Authorization;
  }
  client = new Client({name:'D4C-MCP-check',version:'1.0.0'},{capabilities:{}});
  const options = {requestInit:{headers}};
  transport = job.transport==='sse' ? new SSEClientTransport(new URL(job.url),options) : new StreamableHTTPClientTransport(new URL(job.url),options);
  await client.connect(transport);
  let cursor, count=0;
  for(let page=0;page<20;page++) {
    const result=await client.listTools(cursor ? {cursor}:{});
    count += result.tools.length;
    cursor=result.nextCursor;
    if(!cursor) break;
    if(page===19) throw new Error('catalog-too-large');
  }
  console.log(JSON.stringify({ok:true,toolCount:count,authUnchanged}));
} catch(error) {
  const text=String(error?.message??'');
  const reason=/401|unauthor|authentication/i.test(text) ? 'Authentication refused; previous settings kept'
    : /403/.test(text) ? 'Access refused; previous settings kept'
    : text==='oauth' ? 'Use native MCP OAuth sign-in before connecting'
    : text==='name-collision' ? 'Name belongs to another endpoint; choose a different name'
    : text==='unsupported-layout' ? 'Custom config include layout requires the native MCP settings flow'
    : text==='missing-secret' ? 'Required credential is missing; previous settings kept'
    : 'MCP initialization/tool discovery failed; previous settings kept';
  console.log(JSON.stringify({ok:false,reason}));process.exitCode=1;
} finally {
  try {await client?.close()} catch {}
  clearTimeout(timeout);
}
