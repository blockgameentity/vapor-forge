#!/usr/bin/env bash
# Two-part egress policy check.
#
# 1. Deny: prohibited destinations and mechanisms may not appear anywhere in the
#    shipped tree, in code, config or scripts. Telemetry, analytics, donation,
#    token-donation and self-update channels are prohibited by policy, so they
#    cannot be reintroduced without this check failing.
# 2. Allow: every URL a *default config* can dial must be on the allowlist. Test
#    fixtures are excluded from part 2 by construction: they are not config
#    defaults and never run outside `cargo test`, which is why the check reads
#    `res/config.default.toml` rather than grepping every literal.
set -euo pipefail
cd "$(dirname "$0")/.."

deny=(
  'luastools'
  'tokeer'
  'bst://'
  'manifestwanted'
  'manifestcode/submit'
  'donat'
  'telemetr'
  'analytics'
  'posthog'
  'mixpanel'
  'sentry.io'
  'google-analytics'
  'discord.com/api/webhooks'
)
status=0
for needle in "${deny[@]}"; do
  # The deny list itself lives in this file, so it cannot be scanned.
  hits=$(grep -rniF -e "$needle" \
              --include='*.rs' --include='*.toml' --include='*.sh' \
              --exclude='check-egress.sh' \
              crates res scripts 2>/dev/null \
         | grep -viE '^[^:]+:[0-9]+:\s*(//|#)' || true)
  if [ -n "$hits" ]; then
    printf 'prohibited mechanism "%s":\n%s\n' "$needle" "$hits"
    status=1
  fi
done

allowlist="scripts/egress-allowlist.txt"
mapfile -t allowed < <(grep -vE '^\s*(#|$)' "$allowlist")
found=()
while IFS= read -r url; do
  ok=0
  for prefix in "${allowed[@]}"; do
    case "$url" in "$prefix"*) ok=1; break ;; esac
  done
  [ "$ok" -eq 0 ] && found+=("@ $url")
# Commented examples are documentation, not defaults, so they are stripped first.
done < <(sed -E 's/#.*$//' res/config.default.toml \
         | grep -oE 'https?://[A-Za-z0-9._~:/?#@!$&*+,;=%-]+' \
         | sed -E 's/[",);]+$//' | sort -u)

if [ "${#found[@]}" -gt 0 ]; then
  printf 'default-config destination(s) not on the allowlist:\n  %s\n' "${found[@]}"
  status=1
fi

[ "$status" -eq 0 ] && echo "egress: ok (no prohibited mechanism; default config dials only allowlisted hosts)"
exit "$status"
