# Runner stress: local chaos and sage release gate

Run Tier 2 locally before an RC; run Tier 3 on **two disposable Linux sage boxes,
four GPUs each**. Save the RC version, seed, elapsed time, run IDs, server logs,
receipts and measurements with the release results. Failures block the RC.

## Tier 2: real SSH, seeded faults (stable Rust)

Prerequisites: Rust stable, OpenSSH client/server, bash, Python 3. On Ubuntu:
`sudo apt-get install -y openssh-server python3; sudo mkdir -p /run/sshd`.
On macOS the harness uses `/usr/sbin/sshd`; no Remote Login service is needed.
The harness supplies isolated keys, homes, sshd listeners and proxies.

From the repository root:

```sh
env -u CLAUDE_CONFIG_DIR -u CODEX_HOME cargo test --locked --bin orx -- --ignored chaos --nocapture
```

Default: 32 concurrent runs, at least 120 seconds of faults, then at most 60
seconds to drain. Each seeded hold lasts 1–60 seconds, so the final hold can
overrun the requested duration by 59 seconds. Assertions run every second.
`CHAOS_SEED` accepts a decimal u64 or the printed `u64:seconds:runs` replay seed.
The composite seed overrides duration/run-count environment variables, so it alone
reproduces the launch scripts, fault order and clock jumps; OS scheduling remains real.
Replay with `CHAOS_SEED=8030613221891662191:120:32` before the command. Scale to the
200-run, six-hour soak:

```sh
CHAOS_SEED=8030613221891662191 CHAOS_RUNS=200 CHAOS_DURATION=21600 env -u CLAUDE_CONFIG_DIR -u CODEX_HOME cargo test --locked --bin orx -- --ignored chaos --nocapture
```

The scheduler shuffles complete cycles of black-hole, pause, latency, bandwidth,
connection reset/refusal, sshd restart, engine SIGKILL/restart and independent
wall/monotonic clock jumps. Payloads exit nonzero, SIGKILL themselves, double-fork
through setsid and leave daemons, pin every CPU core, or produce fast 2–8 MiB
logs. The worker runs the production Runtime, SSH transport, host program and log collector. The rejected-key
alias has a run in the same Runtime and its real sshd authentication counter is
checked throughout holds, across engine restarts and clock jumps. A prelude
kills engines at the intent/fence/response/handle hooks and swaps a wrapper boot
ID after killing its wrapper and payload, requiring Lost with one notice.
SQLite captures every phase transition, including transitions between assertions;
checks enforce monotonic phase, immutable terminal phase, one terminal notice,
one payload start and stable `started`, at most two SSH clients per host, no
post-failure auth, a kept-log manifest for every terminal run with an exit receipt,
and byte equality for every collected range. A host live and continuously reachable for 35 seconds must reconcile receipts older than 35 seconds.
After the final clean reconnect, each run must reconcile within 60 seconds of that reconnect
or its receipt's publication, whichever is later. Payloads keep their natural lifetimes;
the drain is bounded by their known maximum lifetime plus 60 seconds. This is opt-in; ordinary tests include
all chaos code in CI but leave its two tests ignored. Seeded frame, receipt
and bootstrap-stub properties run in the ordinary suite.

## Tier 3: sage boxes, per RC

Prerequisites on each box: cgroup v2 and systemd, root/sudo, `stress-ng`, `iproute2`,
Python 3, sysstat, NVIDIA driver and `nvidia-smi`. Keep a console or separate management
route available for the network and reboot cases. Use isolated test accounts,
no valuable workloads, and an RC `orx` on the laptop. Register each SSH alias in
Compute; Connect successfully. Create a local project, then use the commands
below to create 32 experiments. Set `PROJECT` to that project's ID and `HOST`
to the sage SSH alias.
Use the same project on both boxes, in separate rounds.

```sh
# On each sage box:
sudo apt-get install -y stress-ng iproute2 python3 sysstat
stat -fc %T /sys/fs/cgroup                 # cgroup2fs
nvidia-smi -L                             # four GPUs
# On the laptop, in the experiment checkout (include stress.sh in its source):
export HOST=sage-a PROJECT=replace-with-project-id
cat > stress.sh <<'SH'
printf 'started %s\n' "$(date -Is)"
stress-ng --cpu 1 --cpu-load 10 --timeout 10m
printf 'finished %s\n' "$(date -Is)"
SH
orx create-experiment "$PROJECT" --baseline --title 'Stress root' --run-command 'bash stress.sh' > stress-root.txt
ROOT_EXP=$(awk '$1 == "id:" {print $2}' stress-root.txt)
: > experiments.txt
for i in $(seq 1 32); do
  orx create-experiment "$PROJECT" --parent "$ROOT_EXP" --title "Stress $i" --run-command 'bash stress.sh' |
    awk '$1 == "id:" {print $2}' >> experiments.txt
done
test "$(wc -l < experiments.txt)" -eq 32
while IFS= read -r exp; do orx exp run "$exp" --force --backend ssh --host "$HOST" --no-container; done < experiments.txt
orx runs "$PROJECT"
```

**(a) 32 concurrent runs.** Repeat on sage-b. While all 32 are running, sample
below once per second for a minute (run in a separate foreground terminal).
For idle-host overhead, repeat with `sleep 600` payloads, and keep Compute closed.

```sh
# On the sage box; RSS is KiB, ps %CPU is lifetime average (also record interval CPU):
ps -eo pid,ppid,rss,pcpu,args | grep '[/]orx-host-.* \(serve\|wrap\)'
# Binary names contain a digest, so use full command matching for interval samples:
pids=$(pgrep -d, -f '/orx-host-[^ ]+ (serve|wrap)')
pidstat -u -r -p "$pids" 1 60
```

Pass: every wrapper RSS <2048 KiB, `serve` interval CPU <0.5% of one core,
≤2 laptop SSH clients for this alias, no duplicate starts, all results correct.
In the app, inspect run status and logs. CLI launches have no subscribed chat;
for the notice check, launch from a chat and require exactly one completion in
that subscribed chat.
`orx runs "$PROJECT"` and `orx logs RUN_ID` must agree with the app and remote log.

**(b) OOM, payload then wrapper.** On the box, create a limited cgroup using
systemd (the test account needs noninteractive sudo for the cgroup move):

```sh
sudo systemd-run --unit=orx-payload-limit -p Delegate=yes \
  -p MemoryMax=96M -p MemorySwapMax=0 -p OOMPolicy=continue sleep infinity
```

Replace stress.sh with the following and set the experiment command to
`exec bash stress.sh`, so Python becomes the wrapper's direct child. Rerun it:

```sh
cg=$(systemctl show orx-payload-limit -p ControlGroup --value)
printf '%s\n' "$$" | sudo -n tee "/sys/fs/cgroup$cg/cgroup.procs" >/dev/null
exec python3 -c 'x=bytearray(512*1024*1024)'
```

Pass: Failed with SIGKILL in exit.json and a matching kernel cgroup OOM record.
Then `sudo systemctl stop orx-payload-limit` on the box.
For the wrapper, start `sleep 20`, select its PID from that run's wrapper.json,
then on the box move just that wrapper into a memory-limited cgroup:

```sh
WRAPPER_PID=replace-with-wrapper-pid
sudo systemd-run --unit=orx-wrapper-oom \
  -p Delegate=yes \
  sh -c '
    base=/sys/fs/cgroup$(awk -F: '\''$1=="0" {print $3}'\'' /proc/self/cgroup)
    mkdir "$base/supervisor"
    echo $$ > "$base/supervisor/cgroup.procs"
    echo +memory > "$base/cgroup.subtree_control"
    mkdir "$base/victim"
    echo 4096 > "$base/victim/memory.max"
    echo 0 > "$base/victim/memory.swap.max"
    echo "$1" > "$base/victim/cgroup.procs"
    # Existing pages retain their old charge; the next receipt write must fault new pages.
    sleep 60
    # Keep this unit alive until the kernel OOM record has been inspected.
    while :; do sleep 60; done
  ' sh "$WRAPPER_PID"
```

Confirm the wrapper actually died and the kernel logged the cgroup OOM
(`sudo journalctl -k --since '-5 min' | grep -i oom`); match the wrapper PID
and victim cgroup. A unit stop or SIGKILL without this record is not an OOM pass.
Only after recording it, run `sudo systemctl stop orx-wrapper-oom`. Pass: Running +
Unreconciled, no respawn, no fabricated Lost/verified cancel. Abandon after
recording evidence; explicitly kill the disposable payload and descendants.

**(c) One hour of loss/latency.** On the box, choose the interface used by the
laptop route. These commands impair that interface, including SSH.

```sh
export IFACE=eth0
sudo systemd-run --unit=orx-netem sh -c '
  trap '\''tc qdisc del dev "$1" root 2>/dev/null || true'\'' EXIT HUP INT TERM
  tc qdisc replace dev "$1" root netem delay 2000ms loss 30%
  sleep 3600
  tc qdisc del dev "$1" root
  trap - EXIT HUP INT TERM
' sh "$IFACE"
```

Pass: contact becomes Unreachable without terminalizing live work; after the
route returns, completed runs reconcile and logs match remote bytes. No extra
payload starts, no missing/duplicate completion notices. Check both long-running
and already-finished runs. Remove the qdisc from the console if disconnected.

**(d) Laptop sleep.** Start ten-minute runs, close the lid/suspend the laptop
for eight hours, then wake with the app open. Pass: correct phases and complete
logs within 60 seconds without a click; one completion per subscribed chat.

**(e) Host reboot.** Start long runs and, from the box's console:

```sh
sudo systemctl reboot
```

Pass: each interrupted run is Lost with reason `host rebooted`, exactly one
terminal notice, no relaunch. New runs still work after the host returns.

**(f) Hung GPU probe.** Open Compute with a live run; stop only an actual
nvidia-smi child of the host `serve` process (check PPID/args first):

```sh
ps -eo pid,ppid,args | grep '[n]vidia-smi'
PROBE_PID=replace-with-confirmed-probe-pid
kill -STOP "$PROBE_PID"
sleep 10
kill -KILL "$PROBE_PID" 2>/dev/null || true
```

If sampling is too quick, use a private shim in the test account's noninteractive
SSH PATH. On the sage box (bash test account; back up its existing .bashrc):

```sh
cp ~/.bashrc ~/.bashrc.orx-stress-backup
mkdir -p ~/.orx-stress-bin
printf '#!/bin/sh\nkill -STOP $$\n' > ~/.orx-stress-bin/nvidia-smi
chmod +x ~/.orx-stress-bin/nvidia-smi
{ printf 'export PATH="$HOME/.orx-stress-bin:$PATH"\n'; cat ~/.bashrc.orx-stress-backup; } > ~/.bashrc
```

From the laptop, `ssh "$HOST" 'command -v nvidia-smi'` must print the shim path.
Close the test account's SSH master and Connect once in the app, then open
Compute. Pass: probe times out within five seconds; payload, feed, logs and
completion remain responsive. Restore on the box and reconnect:

```sh
mv ~/.bashrc.orx-stress-backup ~/.bashrc
rm -rf ~/.orx-stress-bin
```

**(g) Real password + TOTP.** Use a dedicated sshd/account with PAM password and
TOTP configured by the box administrator; the harness key fixtures do not cover
this. Disable usable keys for its alias. Tail `sudo journalctl -fu ssh` (or sshd)
in a console. Submit, leave untouched for 30 minutes, restart the app, then
Connect once and answer password/TOTP. Kill the master and wait another 30
minutes, including laptop sleep/wake. Pass: NeedsAuth and zero background
attempts after the initial failure; one explicit Connect restores the feed;
master loss returns to NeedsAuth without a password/TOTP retry storm. Count
login connections, not public-key probes or individual MFA factors.

**(h) Resource overload matrix.** Use a dedicated test account and a root console
outside its slice. Run each row separately with 32 active experiments; retain
`journalctl -u orx-...` and kernel logs. PyTorch with CUDA must already be installed
for the GPU row. The pressure services run as the test user in its slice:

```sh
# Root management console on the sage box; use the account behind $HOST.
TEST_USER=orx-stress
TEST_UID=$(id -u "$TEST_USER")
SLICE=user-$TEST_UID.slice
```

For **each row**, sample the app's live logs and Compute view throughout pressure.
On the laptop, count clients and time a request through the existing engine:

```sh
ps -axo comm=,args= | awk -v host="$HOST" '$1 ~ /ssh$/ && index($0,host) {print}'
timeout 35s orx compute test ssh --host "$HOST"
orx runs "$PROJECT"
```

Use GNU `timeout` on the Linux observing laptop; on macOS use the app's request
network timing. Pass for every row: ≤2 SSH clients for the alias; `serve` answers
within its 35-second deadline; no Done without a readable exit.json; an uncertain
outcome is Failed with evidence or Unreconciled, never fabricated success or Lost.
After pressure lifts, existing runs and logs reconcile within 60 seconds without
a new launch or duplicate start; evidence-backed terminal runs have one notice
per subscribed chat. A pressure-induced wrapper death must stay Unreconciled.

| Pressure | Additional evidence and pass rule |
| --- | --- |
| CPU + RAM | CPU saturation and cgroup memory pressure are real; SSH and `serve` meet the common deadline. OOM victims agree with receipts/kernel logs. |
| Disk full | Run-root `df` has zero available bytes, receipt publication encounters ENOSPC, and no completion is invented; after freeing space the original receipt and exact log appear. |
| Tasks + file descriptors | The slice reaches its task limit and the hog reaches EMFILE; the existing connection survives, no reconnect/fork storm, recovery after restoring limits. |
| Four GPUs | All four GPUs report sustained utilization; CPU-side transport, status and receipt collection meet the common deadline even if GPU telemetry times out. |

CPU and RAM (the unit's 90% memory limit keeps pressure inside the test slice):

```sh
sudo systemd-run --unit=orx-cpu-memory --uid="$TEST_USER" --slice="$SLICE" \
  -p MemoryMax=90% -p MemorySwapMax=0 -p OOMPolicy=continue \
  stress-ng --cpu 0 --vm 2 --vm-bytes 90% --timeout 120s
sudo systemctl status orx-cpu-memory
sudo journalctl -k --since '-5 min' | grep -i oom
```

Disk full: reserve at least 5 GiB on the backing filesystem and use a fresh
dedicated filesystem so this cannot fill other users' filesystems. Configure its parent as the SSH remote root **before** launching this
round (from the laptop: `orx compute configure ssh --host "$HOST" --remote-root
/mnt/orx-stress`). Start `sleep 20` runs before starting the fill service.

```sh
sudo truncate -s 4G /var/tmp/orx-stress.img
sudo mkfs.ext4 -F /var/tmp/orx-stress.img
sudo mkdir -p /mnt/orx-stress
sudo mount -o loop /var/tmp/orx-stress.img /mnt/orx-stress
sudo chown "$TEST_USER" /mnt/orx-stress
# Connect and start runs now; they create /mnt/orx-stress/engine/runs.
sudo systemd-run --unit=orx-disk-full --uid="$TEST_USER" --slice="$SLICE" sh -c '
  fill=/mnt/orx-stress/engine/.pressure-fill
  trap '\''rm -f "$fill"'\'' EXIT HUP INT TERM
  free=$(df -B1 --output=avail /mnt/orx-stress | tail -n 1)
  fallocate -l "$((free - 1048576))" "$fill"
  dd if=/dev/zero of="$fill" bs=4096 oflag=append conv=notrunc status=none || true
  df -B1 /mnt/orx-stress
  sleep 90
  rm -f "$fill"
'
# After recovery and evidence collection, reset the host's remote root on the laptop:
# orx compute configure ssh --host "$HOST" --clear remote-root
# Then on the box:
sudo umount /mnt/orx-stress
sudo rm /var/tmp/orx-stress.img
```

Tasks and descriptors: keep the root console outside the constrained slice so
restoring the limit cannot itself need a slot in that slice. Start the service
before lowering TasksMax. Its file descriptors are intentionally held open.

```sh
cat > /tmp/orx-fd-tasks.py <<'PY'
import os, resource, threading, time
resource.setrlimit(resource.RLIMIT_NOFILE, (4096, 4096))
fds = []
try:
    while True:
        fds.append(os.open('/dev/null', os.O_RDONLY))
except OSError as error:
    print('fd pressure:', error, 'open=', len(fds), flush=True)
time.sleep(5)
try:
    while True:
        threading.Thread(target=time.sleep, args=(120,), daemon=True).start()
except RuntimeError as error:
    print('task pressure:', error, flush=True)
time.sleep(120)
PY
sudo systemd-run --unit=orx-fd-tasks --uid="$TEST_USER" --slice="$SLICE" python3 /tmp/orx-fd-tasks.py
TASKS_BEFORE=$(systemctl show "$SLICE" -p TasksMax --value)
TASKS_NOW=$(systemctl show "$SLICE" -p TasksCurrent --value)
sudo systemctl set-property --runtime "$SLICE" TasksMax="$((TASKS_NOW + 8))"
sleep 120
sudo systemctl set-property --runtime "$SLICE" TasksMax="$TASKS_BEFORE"
sudo systemctl stop orx-fd-tasks
sudo journalctl -u orx-fd-tasks --no-pager
rm /tmp/orx-fd-tasks.py
```

Four-GPU matmul (use the Python environment containing CUDA PyTorch):

```sh
cat > /tmp/orx-gpu-pressure.py <<'PY'
import subprocess, sys, torch
assert torch.cuda.device_count() >= 4
worker = '''
import sys,time,torch
torch.cuda.set_device(int(sys.argv[1]))
a=torch.randn((8192,8192),device='cuda',dtype=torch.float16)
b=torch.randn_like(a); c=torch.empty_like(a)
end=time.monotonic()+120
while time.monotonic()<end:
    torch.mm(a,b,out=c)
    torch.cuda.synchronize()
'''
children=[subprocess.Popen([sys.executable,'-c',worker,str(gpu)]) for gpu in range(4)]
assert all(child.wait()==0 for child in children)
PY
CUDA_PYTHON=/absolute/path/to/cuda-venv/bin/python
sudo systemd-run --unit=orx-gpu-pressure --uid="$TEST_USER" --slice="$SLICE" \
  "$CUDA_PYTHON" /tmp/orx-gpu-pressure.py
nvidia-smi --query-gpu=index,utilization.gpu,memory.used --format=csv -l 1
# Ctrl-C only the observer after 120 seconds; the pressure service ends itself.
sudo journalctl -u orx-gpu-pressure --no-pager
rm /tmp/orx-gpu-pressure.py
```
