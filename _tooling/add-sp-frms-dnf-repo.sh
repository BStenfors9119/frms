#!/usr/bin/env bash
# add-sp-frms-dnf-repo.sh — register the SodaPop DNF repo's `frms` product on a
# Fedora machine so `sudo dnf install frms` works and updates flow through
# `dnf upgrade`.
#
# Per-product repo: frms gets its own .repo file and baseurl, separate from any
# future SodaPop products (matching the apt-side convention).
#
#   curl -fsSL https://dnf.sodapopsystems.com/add-sp-frms-dnf-repo.sh | bash
#
# Debian/Ubuntu users: use _tooling/add-sp-frms-repo.sh (apt) instead.

set -euo pipefail

ARCH="$(uname -m)"
if [ "$ARCH" != "x86_64" ]; then
    echo "frms ships x86_64 only; this machine reports '$ARCH'. Aborting." >&2
    exit 1
fi

REPO_FILE="/etc/yum.repos.d/sp-frms.repo"
PUBLIC_URL="https://dnf.sodapopsystems.com/frms"
VPN_URL="http://10.24.3.9:8090/frms"

sudo tee "$REPO_FILE" >/dev/null <<EOF
[sp-frms]
name=SodaPop Systems — frms
baseurl=$PUBLIC_URL
enabled=1
gpgcheck=0
metadata_expire=1h

[sp-frms-vpn]
name=SodaPop Systems — frms (VPN)
baseurl=$VPN_URL
enabled=0
gpgcheck=0
metadata_expire=1h
EOF

echo "Added SodaPop 'frms' DNF repo ($REPO_FILE)."
echo "  Public:  $PUBLIC_URL  (via dnf.sodapopsystems.com)"
echo "  VPN:     $VPN_URL  (enable with: sudo dnf config-manager setopt sp-frms-vpn.enabled=1)"
echo
echo "Next:  sudo dnf install frms"
