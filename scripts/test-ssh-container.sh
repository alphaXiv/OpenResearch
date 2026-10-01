#!/usr/bin/env bash
set -euo pipefail
# Only the uniquely named test containers are changed or removed.
fixture=$(mktemp -d)
prefix="orx-ssh-test-$$"
cleanup() {
  docker rm -f "$prefix-host" "$prefix-inner" >/dev/null 2>&1 || true
  docker image rm "$prefix" >/dev/null 2>&1 || true
  rm -rf "$fixture"
}
trap cleanup EXIT
ssh-keygen -q -t ed25519 -N '' -f "$fixture/key"
cat > "$fixture/Dockerfile" <<'DOCKER'
FROM docker:28-cli
RUN apk add --no-cache openssh bash util-linux coreutils procps && ssh-keygen -A && mkdir -p /root/.ssh && chmod 700 /root/.ssh
COPY key.pub /root/.ssh/authorized_keys
COPY ssh-dispatch /usr/local/bin/ssh-dispatch
RUN chmod +x /usr/local/bin/ssh-dispatch && echo "ForceCommand /usr/local/bin/ssh-dispatch" >> /etc/ssh/sshd_config
RUN chmod 600 /root/.ssh/authorized_keys
ENTRYPOINT ["/usr/sbin/sshd", "-D", "-e"]
DOCKER
cat > "$fixture/ssh-dispatch" <<'DISPATCH'
#!/bin/sh
case "$SSH_ORIGINAL_COMMAND" in
  *'date +%s > launch_time'*)
    if [ -f /tmp/drop-launch-ack ]; then
      rm /tmp/drop-launch-ack
      sh -c "$SSH_ORIGINAL_COMMAND"
      exit 255
    fi ;;
esac
exec sh -c "$SSH_ORIGINAL_COMMAND"
DISPATCH
docker build -q -t "$prefix" "$fixture"
docker run -d --name "$prefix-host" -p 127.0.0.1::22 -v /var/run/docker.sock:/var/run/docker.sock "$prefix"
docker run -d --name "$prefix-inner" --user 1000:1000 --env CONDA_ENVS_PATH=/tmp/conda-envs --env "HOME=/tmp/orx home'quoted" --entrypoint bash condaforge/miniforge3 -c 'mkdir -p "$HOME"; exec sleep infinity'
port=$(docker port "$prefix-host" 22/tcp)
export ORX_SSH_TEST_PORT="${port##*:}" ORX_SSH_TEST_KEY="$fixture/key" ORX_SSH_TEST_CONTAINER="$prefix-inner"
for attempt in {1..30}; do
  if ssh -i "$fixture/key" -p "$ORX_SSH_TEST_PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes root@127.0.0.1 true 2>/dev/null; then break; fi
  sleep 1
done
cargo build --locked
cargo test --locked ssh_container_lifecycle -- --ignored --nocapture
