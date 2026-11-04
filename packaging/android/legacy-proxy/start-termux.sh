#!/data/data/com.termux/files/usr/bin/bash
# Horizon 26.3 -> ViaProxy -> Minecraft Java 1.8.9, all on the same phone.
# This launcher is intentionally separate from Horizon's APK; no Java runtime
# or third-party GPL binary is bundled with Horizon's native/MIT source.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: bash start-termux.sh <minecraft-1.8.9-server[:port]>
Example: bash start-termux.sh mc.example.org:25565

Run this in Termux from F-Droid/GitHub, not Android's generic shell.
Requires internet for the first-time downloads (~150 MB+).
The proxy remains in the foreground. Stop with Ctrl+C.
After it reports Started, open Horizon Android -> Multiplayer ->
Direct Connect -> 127.0.0.1:25568.

IMPORTANT: Offline-mode backend servers are the first test target.
Online-mode premium servers also require licensed Microsoft session
credentials configured securely in ViaProxy; DO NOT paste a password here.
EOF
}
if [[ $# -ne 1 || "$1" == "--help" || "$1" == "-h" ]]; then
  usage
  [[ $# -eq 1 && ( "$1" == "--help" || "$1" == "-h" ) ]] && exit 0 || exit 2
fi
TARGET=$1
# Deliberately disallow shell escapes and unusual YAML characters in targets.
if [[ ! "$TARGET" =~ ^[A-Za-z0-9_.-]+(:[0-9]{1,5})?$ ]]; then
  echo "Invalid server address; use host.example.org[:25565]." >&2
  exit 2
fi
if [[ "$TARGET" == 127.0.0.1* || "$TARGET" == localhost* ]]; then
  echo "Refusing a loopback target; that would create a proxy loop." >&2
  exit 2
fi
if [[ "${PREFIX:-}" != *"/com.termux/"* ]]; then
  echo "This helper is intended for the official Termux Android environment." >&2
  exit 2
fi
for package in curl jq openjdk-25; do
  binary=$package
  [[ "$package" == openjdk-25 ]] && binary=java
  if ! command -v "$binary" >/dev/null 2>&1; then
    echo "Installing Termux package $package..."
    pkg install -y "$package"
  fi
done

APP_DIR="$HOME/.local/share/horizon-189-proxy"
mkdir -p "$APP_DIR"
cd "$APP_DIR"
if [[ ! -s ViaProxy.jar ]]; then
  echo "Finding current official ViaProxy release..."
  metadata=$(curl -fLsS --retry 3 --connect-timeout 20 \
    -H 'Accept: application/vnd.github+json' \
    'https://api.github.com/repos/ViaVersion/ViaProxy/releases/latest')
  url=$(printf '%s' "$metadata" | jq -r \
    '[.assets[] | select(.name | test("\\.jar$"; "i")) | select(.name | test("(java8|jdk8)"; "i") | not) | .browser_download_url][0] // empty')
  if [[ -z "$url" || "$url" != https://github.com/ViaVersion/ViaProxy/releases/download/* ]]; then
    echo "No official ViaProxy JAR found; download one manually from" >&2
    echo "https://github.com/ViaVersion/ViaProxy/releases/latest" >&2
    echo "and save it as $APP_DIR/ViaProxy.jar" >&2
    exit 1
  fi
  curl -fL --retry 3 --connect-timeout 20 -o ViaProxy.jar.tmp "$url"
  [[ -s ViaProxy.jar.tmp ]] || { echo 'Empty download' >&2; exit 1; }
  mv ViaProxy.jar.tmp ViaProxy.jar
fi

if [[ ! -f viaproxy.yml ]]; then
  echo 'Generating official ViaProxy config...'
  java -Djava.awt.headless=true -Xmx256m -jar ViaProxy.jar config viaproxy.yml
fi
[[ -f viaproxy.yml ]] || { echo 'ViaProxy config was not created' >&2; exit 1; }

# Update only these four keys; keep the ViaProxy-generated config defaults.
# Restrict bind to localhost, so strangers on Wi-Fi cannot enter the proxy.
awk -v target="$TARGET" '
  BEGIN {seen_bind=0; seen_target=0; seen_ver=0; seen_auth=0}
  /^bind-address:/       {print "bind-address: 127.0.0.1:25568"; seen_bind=1; next}
  /^target-address:/     {print "target-address: " target; seen_target=1; next}
  /^target-version:/     {print "target-version: 1.8.x"; seen_ver=1; next}
  /^auth-method:/        {print; seen_auth=1; next}
                         {print}
  END {
    if (!seen_bind) print "bind-address: 127.0.0.1:25568"
    if (!seen_target) print "target-address: " target
    if (!seen_ver) print "target-version: 1.8.x"
    if (!seen_auth) print "auth-method: NONE"
  }
' viaproxy.yml > viaproxy.yml.tmp
mv viaproxy.yml.tmp viaproxy.yml

# DO NOT force auth-method back to NONE on every start: the user may have set
# up an account in their PRIVATE config for authenticated servers.
echo "Proxy local: 127.0.0.1:25568  ->  1.8.9 target: $TARGET"
echo "Switch to Horizon Android and use 127.0.0.1:25568."
echo 'Termux must remain running. Press Ctrl+C to stop.'
exec java -Djava.awt.headless=true -Xms64m -Xmx384m -jar ViaProxy.jar start viaproxy.yml
