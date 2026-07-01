#!/usr/bin/env bash
# add-sp-frms-repo.sh — register the SodaPop apt repo's `frms` component on a
# Debian/Ubuntu/Mint machine so `sudo apt install frms` works and updates flow
# through `apt update`.
#
# frms gets its OWN sources file and its OWN component (`frms`), kept separate
# from the TSR receivers' `main` and the cameras' `hqcam`. Run once per machine.
#
#   curl -fsSL <url>/add-sp-frms-repo.sh | bash      # or download + run
#
# Fedora users do NOT use this — install the .rpm from GitHub Releases instead
# (there is no self-hosted dnf repo).
#
# Keyring flow — modern (deb822 .sources + Signed-By):
#   The legacy `apt-key` is removed on Debian 12 / Ubuntu 24.04, and the old
#   one-line `deb [signed-by=…trusted.gpg]` form is superseded by the deb822
#   `.sources` format (apt ≥ 2.4). We write a `.sources` stanza referencing a
#   dedicated keyring under /etc/apt/keyrings — the FHS-blessed location, never
#   /etc/apt/trusted.gpg.d (global trust). Multiple URIs (VPN + public) act as
#   mirrors for the same suite, replacing the old two-`deb`-lines hack.

set -euo pipefail

ARCH="$(dpkg --print-architecture)"
if [ "$ARCH" != "amd64" ]; then
    echo "frms ships amd64 only; this machine reports '$ARCH'. Aborting." >&2
    exit 1
fi

# Same repo host/key as TSR + hqcam. VPN host first (internal), public second —
# deb822 treats them as mirrors of one repo.
PUBLIC_APT_URL="apt.sodapopsystems.com"
KEYRING="/etc/apt/keyrings/sodapop-archive-keyring.gpg"
SOURCES="/etc/apt/sources.list.d/sp-frms.sources"

sudo install -d -m 0755 /etc/apt/keyrings
echo "Acquire::By-Hash=yes;" | sudo tee /etc/apt/apt.conf.d/99acquire-by-hash >/dev/null

# Fetch the shared repo public key (armored) and dearmor it into a dedicated
# keyring. Idempotent — re-running overwrites with the same key. One shared key
# signs the whole dist, so this same keyring also verifies `main`/`hqcam`.
if [ ! -s "$KEYRING" ]; then
    curl -fsSL "https://${PUBLIC_APT_URL}/apt-repo/pubkey.gpg" \
        | sudo gpg --dearmor -o "$KEYRING"
    sudo chmod 0644 "$KEYRING"   # readable by the unprivileged _apt fetcher
fi

# deb822 stanza — one repo, two mirror URIs, just the `frms` component.
sudo tee "$SOURCES" >/dev/null <<EOF
Types: deb
URIs: https://${PUBLIC_APT_URL}/apt-repo
Suites: stable
Components: frms
Architectures: amd64
Signed-By: ${KEYRING}
EOF

echo "Added the SodaPop 'frms' apt component ($SOURCES, deb822)."
echo "Next:  sudo apt update && sudo apt install frms"
