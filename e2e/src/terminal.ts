/** A project terminal as the dashboard's socket sees it: output read as text, input as keystrokes. @module */
import type { Orx } from './orx.ts'

// Node's WebSocket takes request headers, which the DOM typings lack; the server checks the Origin.
const NodeWebSocket = WebSocket as unknown as new (url: URL, options: { headers: Record<string, string> }) => WebSocket

/** Attaches to terminal `id`, which the server starts if there is none. */
export async function openTerminal(orx: Orx, projectId: string, id: string) {
  const url = new URL(`/api/projects/${projectId}/terminal?terminalId=${id}`, orx.url.replace('http', 'ws'))
  const socket = new NodeWebSocket(url, { headers: { origin: orx.url } })
  socket.binaryType = 'arraybuffer'
  const decoder = new TextDecoder()
  let output = ''
  socket.onmessage = ({ data }) => {
    // Output is binary; the server's own notices (exit, an error) are JSON text.
    output += typeof data === 'string' ? `\n${data}\n` : decoder.decode(data as ArrayBuffer, { stream: true })
  }
  await new Promise((resolve, reject) => {
    socket.onopen = resolve
    socket.onerror = () => reject(new Error(`Could not open terminal ${id} on ${orx.url}`))
  })
  socket.send(JSON.stringify({ type: 'resize', cols: 200, rows: 50 }))
  return {
    /** Everything received so far, including the replay of what the shell printed before. */
    output: () => output,
    write: (keys: string) => socket.send(new TextEncoder().encode(keys)),
    close: () => socket.close(),
  }
}
