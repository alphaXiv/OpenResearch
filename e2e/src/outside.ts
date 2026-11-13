/**
 * Everything outside the machine, as one loopback server. orx reaches it two ways: as
 * openresearch.sh through `OPENRESEARCH_API_URL`, and as the HTTP(S) proxy every other request
 * goes through, which refuses it. Either way the request is recorded. @module
 */
import { createServer } from 'node:http'
import type { AddressInfo } from 'node:net'

export type Request = { service: 'openresearch' | 'refused'; method: string; target: string }

export type Outside = Awaited<ReturnType<typeof startOutside>>

/** Traffic arc/storage.md's "What leaves the machine" lists for what the specs do: the release check and the onboarding's OpenCode install. */
export function listedEgress({ method, target }: Request) {
  return method === 'CONNECT' && ['github.com:443', 'api.github.com:443'].includes(target)
}

export async function startOutside() {
  const requests: Request[] = []
  const server = createServer((request, response) => {
    const proxied = /^https?:\/\//.test(request.url!)
    requests.push({ service: proxied ? 'refused' : 'openresearch', method: request.method!, target: request.url! })
    response.writeHead(proxied ? 403 : 404, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ error: proxied ? 'e2e refuses traffic off the machine' : 'not faked' }))
  })
  server.on('connect', (request, socket) => {
    requests.push({ service: 'refused', method: 'CONNECT', target: request.url! })
    socket.end('HTTP/1.1 403 Forbidden\r\n\r\n')
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))

  return {
    url: `http://127.0.0.1:${(server.address() as AddressInfo).port}`,
    requests,
    close() {
      server.closeAllConnections()
      return new Promise((resolve) => server.close(resolve))
    },
  }
}
