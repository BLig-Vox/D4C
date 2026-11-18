// D4C screenshot-history projection. Stored transcripts and source PNGs are untouched.
const MiB = 1024 * 1024;
const marker = /Saved screenshot:\s*[^\n]*[\\/]d4c-screenshots[\\/]d4c-[a-f0-9]{16}-[a-f0-9-]{36}\.png\b/;
const omitted = '[Older D4C desktop screenshot pixels omitted from this model request to bound image history. The recorded tool result and its saved-file reference remain in history. Use desktop_screenshot to inspect the current screen; do not infer current state from this marker.]';
export function projectScreenshotHistory(body) {
  if (!body || typeof body !== 'object' || !Array.isArray(body.input)) return body;
  const results = [];
  for (let index = 0; index < body.input.length; index++) {
    const item = body.input[index];
    if (item?.type !== 'function_call_output' || !Array.isArray(item.output)) continue;
    if (!item.output.some(b => b?.type === 'input_text' && typeof b.text === 'string' && marker.test(b.text))) continue;
    const images = item.output.filter(b => b?.type === 'input_image' && typeof b.image_url === 'string' && b.image_url.startsWith('data:image/'));
    if (images.length) results.push({index, images, bytes: images.reduce((n,b) => n + Buffer.byteLength(JSON.stringify(b)), 0)});
  }
  if (results.length === 0) return body;
  // Keep the newest observation even when it alone exceeds the preferred image
  // budget. The original downstream body cap still rejects anything too large.
  const keep = new Set();
  let bytes = 0, count = 0;
  for (let i = results.length - 1; i >= 0; i--) {
    const r = results[i];
    if (keep.size === 0 || (count + r.images.length <= 4 && bytes + r.bytes <= 4 * MiB)) {
      keep.add(r.index); bytes += r.bytes; count += r.images.length;
    }
  }
  if (keep.size === results.length) return body;
  const eligible = new Set(results.map(r => r.index));
  const input = body.input.map((item,index) => {
    if (!eligible.has(index) || keep.has(index)) return item;
    return {...item, output:item.output.map(block => block?.type === 'input_image' && typeof block.image_url === 'string' && block.image_url.startsWith('data:image/')
      ? {type:'input_text',text:omitted} : block)};
  });
  console.info(`[d4c-image-budget] omitted=${results.length-keep.size} kept=${keep.size} retainedImageBytes=${bytes}`);
  return {...body,input};
}
