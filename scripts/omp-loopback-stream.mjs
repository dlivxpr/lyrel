// Public OpenAI Chat Completions SSE, consumed by released omp's own renderer.
// This never renders SVG or writes graphics bytes; only the native agent does.
import { existsSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

export async function streamSvg(request, response, { root, stage, requests }) {
  const save = () => writeFileSync(join(root, 'requests.json'), JSON.stringify(requests, null, 2));
  const entry = { method: request.method, url: request.url, peer: request.socket.remoteAddress };
  requests.push(entry);
  save();
  if (requests.length !== 1 || entry.peer !== '127.0.0.1' || entry.method !== 'POST' || entry.url !== '/v1/chat/completions') {
    throw new Error('Only one owned loopback chat-completions request is permitted');
  }
  let bytes = 0;
  const chunks = [];
  for await (const chunk of request) {
    bytes += chunk.length;
    if (bytes > (512 << 10)) throw new Error('Unexpectedly large fixture request');
    chunks.push(chunk);
  }
  const body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  entry.model = body.model;
  entry.stream = body.stream;
  entry.promptMatched = JSON.stringify(body.messages?.at(-1)?.content).includes('LYREL_LOOPBACK_PROMPT');
  save();
  if (entry.model !== 'stream-svg' || entry.stream !== true || !entry.promptMatched) throw new Error('Unexpected provider request');
  response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache', Connection: 'keep-alive' });
  response.flushHeaders();
  const frame = (delta, finish_reason = null) => response.write(`data: ${JSON.stringify({
    id: 'lyrel-owned-stream', object: 'chat.completion.chunk', created: 1, model: 'stream-svg',
    choices: [{ index: 0, delta, finish_reason }],
    ...(finish_reason ? { usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } } : {}),
  })}\n\n`);
  const gate = async name => {
    const deadline = Date.now() + 45000;
    while (!existsSync(join(root, name))) {
      if (response.destroyed) throw new Error(`Consumer disconnected at ${name}`);
      if (Date.now() >= deadline) throw new Error(`Production consumer did not acknowledge ${name}`);
      await new Promise(resolve => setTimeout(resolve, 20));
    }
  };
  frame({ role: 'assistant', content: 'LYREL_STREAM_BEFORE\n```svg\n<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32" viewBox="0 0 64 32"><rect width="64" height="32" fill="#ff0000"/>\n' });
  stage('raster-1');
  await gate('observed-1');
  frame({ content: '<rect ' });
  stage('partial-1');
  await gate('partial-observed-1');
  frame({ content: 'x="32" width="32" height="32" fill="#00ff00"/>\n' });
  stage('raster-2');
  await gate('observed-2');
  frame({ content: '<rect ' });
  stage('partial-2');
  await gate('partial-observed-2');
  frame({ content: 'y="16" width="64" height="16" fill="#0000ff"/></svg>\n```\nLYREL_STREAM_AFTER' });
  stage('raster-3');
  await gate('observed-3');
  frame({}, 'stop');
  response.end('data: [DONE]\n\n');
  entry.completed = true;
  save();
  stage('stream-complete');
}
