import { createFileRoute } from '@tanstack/react-router'

export const Route = createFileRoute('/remote-launch')({
  component: RouteComponent,
})

function RouteComponent() {
  return <div>Hello "/remote-launch"!</div>
}
