// Minimal WebDriver BiDi stub. Speaks just enough for the e2e, and deliberately
// interleaves an unsolicited event before each command response so the
// correlation logic in src/demux.rs is actually exercised. A mock that only
// ever replied in order would pass a naive implementation and defeat the point.
import { WebSocketServer } from 'ws';

const port = Number(process.env.PORT || 9222);
const wss = new WebSocketServer({ port, path: '/session' });

// Diagnostic for a CI-only hang investigation (see docs/specs/2026-08-08-
// e2e-harness-findings.md): the raw TCP 'connection' event on the
// underlying http.Server fires on accept, before any WebSocket upgrade is
// attempted — distinct from wss's own 'connection' event below, which only
// fires once the upgrade completes. Logging both tells apart "the guest
// never got a TCP connection accepted" from "TCP connected but the upgrade
// handshake never finished."
wss._server.on('connection', (socket) => {
  console.error(`[mock] TCP accept from ${socket.remoteAddress}:${socket.remotePort}`);
});

// 1x1 transparent PNG.
const PNG_1X1 =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk' +
  'YPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';

wss.on('connection', (ws) => {
  console.error('[mock] WebSocket upgrade completed');
  ws.on('message', (raw) => {
    let msg;
    try {
      msg = JSON.parse(raw.toString());
    } catch {
      return;
    }
    const { id, method, params } = msg;

    // Unsolicited event BEFORE the response — the interleaving case.
    ws.send(
      JSON.stringify({
        type: 'event',
        method: 'log.entryAdded',
        params: { level: 'info', text: `before:${method}`, timestamp: 0 },
      }),
    );

    let result = {};
    switch (method) {
      case 'session.new':
        result = { sessionId: 'mock-session-1', capabilities: {} };
        break;
      case 'session.subscribe':
      case 'session.end':
        result = {};
        break;
      case 'browsingContext.getTree':
        result = { contexts: [{ context: 'ctx-1', url: 'about:blank', children: [] }] };
        break;
      case 'browsingContext.navigate':
        result = { navigation: 'nav-1', url: params.url };
        break;
      case 'browsingContext.create':
        result = { context: 'ctx-2' };
        break;
      case 'browsingContext.close':
        result = {};
        break;
      case 'browsingContext.captureScreenshot':
        result = { data: PNG_1X1 };
        break;
      case 'script.evaluate':
        // resultOwnership: "root" is how resolve_node asks for a node handle.
        result =
          params.resultOwnership === 'root'
            ? { type: 'success', result: { type: 'node', sharedId: 'node-1' } }
            : { type: 'success', result: { type: 'string', value: 'mock text' } };
        break;
      case 'input.performActions':
        result = {};
        break;
      default:
        ws.send(
          JSON.stringify({ type: 'error', id, error: 'unknown command', message: method }),
        );
        return;
    }
    ws.send(JSON.stringify({ type: 'success', id, result }));
  });
});

process.on('SIGTERM', () => {
  wss.close();
  process.exit(0);
});
console.log(`mock-bidi listening on ws://127.0.0.1:${port}/session`);
