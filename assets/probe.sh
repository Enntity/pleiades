# SPDX-License-Identifier: AGPL-3.0-only
# Shrug probe: read-only facts about one host. Output is parsed by src/probe.rs:
# "## name" section headers, then "key=value" lines or positional lines.
set -u
echo '## shrug.probe/v1'
echo '## host'
echo "hostname=$(hostname)"
mid=$(tr -d '[:space:]' </etc/machine-id 2>/dev/null)
[ -z "$mid" ] && mid=$(tr -d '[:space:]' </var/lib/dbus/machine-id 2>/dev/null)
[ -z "$mid" ] && mid="noid-$(hostname)"
echo "machine_id=$mid"
echo "arch=$(uname -m)"
echo "user=$(id -un)"
echo "home=$HOME"
echo '## gpu'
nvidia-smi --query-gpu=name,driver_version --format=csv,noheader 2>/dev/null | head -1
echo '## mem'
awk '/^MemTotal|^MemAvailable/{sub(/:/,"",$1); print tolower($1)"="$2}' /proc/meminfo 2>/dev/null
echo '## fabric'
for dev in /sys/class/infiniband/*; do
  [ -e "$dev" ] || continue
  hca=${dev##*/}
  state=$(awk '{print $2}' "$dev/ports/1/state" 2>/dev/null)
  for net in "$dev"/device/net/*; do
    [ -e "$net" ] || continue
    n=${net##*/}
    addrs=$(ip -4 -o addr show dev "$n" 2>/dev/null | awk '{print $4}' | paste -sd, -)
    echo "$hca $n state=${state:-unknown} addrs=$addrs"
  done
done
echo '## lan'
ip -4 route show default 2>/dev/null | awk '{for(i=1;i<=NF;i++){if($i=="dev")d=$(i+1);if($i=="src")s=$(i+1)};print "lan_dev="d;print "lan_ip="s;exit}'
echo '## docker'
echo "docker_server=$(docker version --format '{{.Server.Version}}' 2>/dev/null)"
docker info >/dev/null 2>&1 && echo docker_ok=1 || echo docker_ok=0
echo "runtimes=$(docker info --format '{{range $k,$_ := .Runtimes}}{{$k}},{{end}}' 2>/dev/null | sed 's/,$//')"
nvidia-ctk --version >/dev/null 2>&1 && echo nvidia_ctk=1 || echo nvidia_ctk=0
echo '## containers'
for id in $(docker ps -q 2>/dev/null); do
  docker inspect -f '{{.Name}}|{{.Config.Image}}|{{.State.Status}}|{{.HostConfig.Runtime}}|{{with .HostConfig.DeviceRequests}}{{with index . 0}}{{.Capabilities}}{{end}}{{end}}' "$id" 2>/dev/null | sed 's|^/||'
done
echo '## gpu-procs'
nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader 2>/dev/null | sed 's/, */|/g'
echo '## clockcap'
systemctl is-active gpu-clock-cap.service 2>/dev/null || echo inactive
echo '## disk'
df -Pk "$HOME" 2>/dev/null | awk 'NR==2{print "home_free_kib="$4}'
echo '## tools'
t=
for c in rsync hf curl ping; do command -v "$c" >/dev/null 2>&1 && t="$t$c,"; done
echo "tools=${t%,}"
