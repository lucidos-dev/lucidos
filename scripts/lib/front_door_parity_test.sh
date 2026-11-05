#!/bin/bash
# Tests for scripts/lib/front_door_parity.sh, the guard that stops the
# production and release-candidate front doors serving different route sets.
#
# The property under test: a route that resolves to shell at production and not
# at the release candidate must FAIL and name the route. That is the 2026-07-30
# shape, where the site publisher's production route discovery learned about
# `/uninstall.sh` and its release-candidate discovery did not, and the omission
# only surfaced the next morning as three red rc-push legs.
#
# What this pins:
#   1. the route set is derived from EACH ORIGIN'S OWN served scripts, never
#      from a hardcoded list and never from this checkout, so a new helper lib
#      is covered the day it ships;
#   2. the uninstaller's own lib fetch is part of that derivation, not covered
#      only by coincidence through install.sh's list;
#   3. the three severities: production-ahead is fatal and names the route,
#      candidate-ahead is a warning (an in-flight candidate legitimately leads),
#      and missing-at-both is a warning (the front-door job owns that verdict);
#   4. every parse fails CLOSED. A comparison that could not be made must never
#      read as "the two origins agree";
#   5. the lib base is compared by EQUALITY, so an installer pinned at the other
#      origin cannot vacuously satisfy the check.
#
# Hermetic and offline: both origins are `file://` trees, so no network, no
# Cloudflare, no live site. Curl reads local files, which exercises the real
# fetch and sniff path rather than a stub. Same shape as
# release_rc_front_door_test.sh, its neighbour.
#
# Run: ./scripts/lib/front_door_parity_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/front_door_parity.sh
source "$SCRIPT_DIR/front_door_parity.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT

PROD_DIR="$ROOT/prod"; PROD="file://$PROD_DIR"
RC_DIR="$ROOT/rc";     RC="file://$RC_DIR"

# The RC origin default is https://rc.*, which fdp_require_access gates on a
# service token. These file:// origins do not match that prefix, so no
# credentials are involved anywhere below. Unset them regardless: an operator
# running this on the release Mac has them exported, and a test whose result
# depends on the caller's environment is not hermetic.
unset RC_ACCESS_CLIENT_ID RC_ACCESS_CLIENT_SECRET

# ── origin fixtures ──────────────────────────────────────────────────────────
# An installer shaped like the real one: a baked LUCIDOS_INSTALL_URL pinned at
# its own origin, a LUCIDOS_LIBS list, and the source_service_lib call that
# declares the launch-path libs.
write_installer() { # <dir> <origin> [<extra-lib>…]
  local dir="$1" origin="$2"; shift 2
  mkdir -p "$dir"
  cat > "$dir/install.sh" <<EOF
#!/usr/bin/env bash
LUCIDOS_INSTALL_URL="\${LUCIDOS_INSTALL_URL:-$origin/install.sh}"
LUCIDOS_DEFAULT_VERSION="0.18.0"
LUCIDOS_LIBS="stage_runtime.sh headless_tarball.sh install_common.sh $*"
source_service_lib() { _source_libs service.sh install_common.sh; }
EOF
}

# An uninstaller shaped like the real one: its own self URL, its own baked
# LUCIDOS_INSTALL_URL, and the literal \$base/<lib>.sh fetch inside
# source_service_lib that install.sh does NOT declare.
write_uninstaller() { # <dir> <origin> [<lib-basename>]
  local dir="$1" origin="$2" lib="${3:-service}"
  mkdir -p "$dir"
  cat > "$dir/uninstall.sh" <<EOF
#!/usr/bin/env bash
LUCIDOS_UNINSTALL_SELF_URL="\${LUCIDOS_UNINSTALL_SELF_URL:-$origin/uninstall.sh}"
LUCIDOS_INSTALL_URL="\${LUCIDOS_INSTALL_URL:-$origin/install.sh}"
source_service_lib() {
    base="\${LUCIDOS_LIB_BASE_URL:-\${LUCIDOS_INSTALL_URL%/install.sh}/scripts/lib}"
    curl -fsSL "\$base/$lib.sh" -o "\$tmp/$lib.sh" || die "no $lib.sh"
}
EOF
}

write_libs() { # <dir> <name…>
  local dir="$1"; shift
  mkdir -p "$dir/scripts/lib"
  local n
  for n in "$@"; do printf '#!/usr/bin/env bash\n: %s\n' "$n" > "$dir/scripts/lib/$n"; done
}

# The soft 404: Cloudflare Pages answers a missing path with the landing page at
# status 200, so only the BODY reveals the truth.
write_soft_404() { # <file>
  cat > "$1" <<'EOF'
<!DOCTYPE html>
<html><head><title>Lucidos</title></head>
<body>If you can describe it, it exists.</body></html>
EOF
}

# A complete, healthy origin: installer, uninstaller, and all four libs.
write_healthy_origin() { # <dir> <origin>
  write_installer "$1" "$2"
  write_uninstaller "$1" "$2"
  write_libs "$1" stage_runtime.sh headless_tarball.sh install_common.sh service.sh
}

reset_origins() {
  rm -rf "$PROD_DIR" "$RC_DIR"
  write_healthy_origin "$PROD_DIR" "$PROD"
  write_healthy_origin "$RC_DIR" "$RC"
}

echo "== 1. the route set comes from each origin's OWN served scripts =="

reset_origins
routes="$(fdp_derive_routes "$PROD" "$(mktemp -d)")"
expected="/install.sh
/uninstall.sh
/scripts/lib/headless_tarball.sh
/scripts/lib/install_common.sh
/scripts/lib/service.sh
/scripts/lib/stage_runtime.sh"
if [ "$(printf '%s\n' "$routes" | sort)" = "$(printf '%s\n' "$expected" | sort)" ]; then
  pass "derives the six publish routes a piped run would fetch"
else
  fail "derived the wrong route set: $(printf '%s' "$routes" | tr '\n' ' ')"
fi

# The candidate declares a lib production has never heard of. If the derivation
# were reading this checkout's install.sh (or a hardcoded list) both origins
# would report the same set, which is the failure this case exists to catch.
write_installer "$RC_DIR" "$RC" "brand_new_lib.sh"
write_libs "$RC_DIR" brand_new_lib.sh
rc_routes="$(fdp_derive_routes "$RC" "$(mktemp -d)")"
if printf '%s\n' "$rc_routes" | grep -qx '/scripts/lib/brand_new_lib.sh' \
   && ! printf '%s\n' "$routes" | grep -qx '/scripts/lib/brand_new_lib.sh'; then
  pass "two origins serving different installers derive different route sets"
else
  fail "the derivation is not reading the served installer (rc: $(printf '%s' "$rc_routes" | tr '\n' ' '))"
fi

# The uninstaller's own \$base/<lib>.sh fetch must join the set. install.sh does
# not declare it, so without this the uninstaller's helper is covered only
# because service.sh happens to be in install.sh's list today.
reset_origins
write_uninstaller "$PROD_DIR" "$PROD" "uninstall_only_lib"
routes="$(fdp_derive_routes "$PROD" "$(mktemp -d)")"
if printf '%s\n' "$routes" | grep -qx '/scripts/lib/uninstall_only_lib.sh'; then
  pass "a lib fetched only by uninstall.sh is part of the route set"
else
  fail "uninstall.sh's own lib fetch was not derived: $(printf '%s' "$routes" | tr '\n' ' ')"
fi

echo "== 2. production ahead of the candidate is FATAL and names the route =="

# The 2026-07-30 shape, reproduced exactly: production serves /uninstall.sh, the
# candidate answers with the landing page at 200.
reset_origins
write_soft_404 "$RC_DIR/uninstall.sh"
out="$(fdp_compare "$PROD" "$RC" 2>&1)"; rc=$?
if [ "$rc" -ne 0 ]; then
  pass "fails when the candidate origin is missing a route production serves"
else
  fail "reported parity while the candidate soft-404s /uninstall.sh"
fi
if printf '%s' "$out" | grep -q '/uninstall.sh'; then
  pass "names the missing route"
else
  fail "the failure does not name the route: $out"
fi
if printf '%s' "$out" | grep -q 'site publisher'; then
  pass "names the cause (the publisher's two route-discovery paths)"
else
  fail "the failure does not point at the publisher: $out"
fi

# A helper lib, not just the uninstaller: the same rule must hold for every
# publish route, since all of them reach a shell.
reset_origins
write_soft_404 "$RC_DIR/scripts/lib/service.sh"
if fdp_compare "$PROD" "$RC" >/dev/null 2>&1; then
  fail "reported parity while the candidate soft-404s a helper lib"
else
  pass "fails on a diverged helper lib too, not only on the uninstaller"
fi

echo "== 3. the candidate ahead of production is a WARNING, not a failure =="

# An in-flight candidate that ADDS a route legitimately leads production, which
# only gets it at publish. Failing here would red the daily cron through every
# such release window.
reset_origins
write_installer "$RC_DIR" "$RC" "brand_new_lib.sh"
write_libs "$RC_DIR" brand_new_lib.sh
out="$(fdp_compare "$PROD" "$RC" 2>&1)"; rc=$?
if [ "$rc" -eq 0 ]; then
  pass "stays green when the candidate serves a route production does not"
else
  fail "redded on a candidate that is merely ahead of production: $out"
fi
if printf '%s' "$out" | grep -q 'WARNING.*brand_new_lib.sh'; then
  pass "warns about the route production will need at publish"
else
  fail "no warning for the production-lacks case: $out"
fi

echo "== 4. missing at BOTH origins is a warning, not a duplicate red =="

# Both front doors broken is the front-door job's verdict to give, and it runs
# against production in the same daily workflow run.
reset_origins
write_soft_404 "$PROD_DIR/uninstall.sh"
write_soft_404 "$RC_DIR/uninstall.sh"
out="$(fdp_compare "$PROD" "$RC" 2>&1)"; rc=$?
if [ "$rc" -eq 0 ]; then
  pass "does not red when the two origins agree, even agreeing on broken"
else
  fail "duplicated the front-door job's verdict: $out"
fi
if printf '%s' "$out" | grep -q 'neither front door serves /uninstall.sh'; then
  pass "still reports the route as missing at both"
else
  fail "silently swallowed a route missing at both: $out"
fi

echo "== 5. two healthy, identical origins pass =="

reset_origins
out="$(fdp_compare "$PROD" "$RC" 2>&1)"; rc=$?
if [ "$rc" -eq 0 ] && printf '%s' "$out" | grep -q 'both front doors serve the same route set'; then
  pass "identical route sets report parity"
else
  fail "a healthy pair did not pass (rc=$rc): $out"
fi

echo "== 6. every parse fails CLOSED =="

# An origin serving nothing at all. This is also the "the RC front door was
# never published, or was reset" case, and the message must say so rather than
# reporting a route divergence.
reset_origins
rm -f "$RC_DIR/install.sh"
out="$(fdp_compare "$PROD" "$RC" 2>&1)"; rc=$?
if [ "$rc" -ne 0 ]; then
  pass "fails when an origin serves no installer at all"
else
  fail "reported parity against an origin with no installer"
fi
if printf '%s' "$out" | grep -q 'push-rc'; then
  pass "points at re-arming the RC front door rather than at a route regression"
else
  fail "unhelpful diagnosis for an unpublished origin: $out"
fi

# The soft-404 installer: status 200, HTML body. `curl -f` cannot see this, and
# it is the exact failure that started all of this.
reset_origins
write_soft_404 "$RC_DIR/install.sh"
if fdp_compare "$PROD" "$RC" >/dev/null 2>&1; then
  fail "accepted a soft-404 landing page as an installer"
else
  pass "rejects an installer that is really the landing page at 200"
fi

# A real shell script whose lib declaration scraped to nothing: the parser
# changed shape, so it must never be read as "no libs to check".
reset_origins
cat > "$RC_DIR/install.sh" <<EOF
#!/usr/bin/env bash
LUCIDOS_INSTALL_URL="\${LUCIDOS_INSTALL_URL:-$RC/install.sh}"
echo hi
EOF
out="$(fdp_compare "$PROD" "$RC" 2>&1)"; rc=$?
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q 'scraped only'; then
  pass "fails closed when the lib scrape finds nothing"
else
  fail "a lib declaration that scraped to nothing did not fail closed: $out"
fi

# No parsable LUCIDOS_INSTALL_URL: the lib base cannot be resolved, so the paths
# probed would not be the paths a piped run fetches.
reset_origins
printf '#!/usr/bin/env bash\nLUCIDOS_LIBS="a.sh b.sh"\n' > "$RC_DIR/install.sh"
if fdp_compare "$PROD" "$RC" >/dev/null 2>&1; then
  fail "passed an installer with no parsable LUCIDOS_INSTALL_URL"
else
  pass "fails closed when LUCIDOS_INSTALL_URL cannot be parsed"
fi

# The uninstaller's pin is parsed just as strictly as the installer's: it is
# where its own service.sh comes from, and install.sh does not export its copy
# before exec-ing the delegated uninstaller.
reset_origins
printf '#!/usr/bin/env bash\necho no pins here\n' > "$RC_DIR/uninstall.sh"
if fdp_compare "$PROD" "$RC" >/dev/null 2>&1; then
  fail "passed an uninstaller with no parsable LUCIDOS_INSTALL_URL"
else
  pass "fails closed on an unparsable pin in the served uninstaller"
fi

echo "== 7. the lib base is compared by EQUALITY, not by prefix =="

# An installer served at one origin but pinned at another fetches its libs from
# there, so this origin's script paths were never exercised. A prefix match was
# vacuous the moment two origins shared an apex.
reset_origins
write_installer "$RC_DIR" "$PROD"
out="$(fdp_compare "$PROD" "$RC" 2>&1)"; rc=$?
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q 'points its helper-lib fetch at'; then
  pass "refuses an installer pinned at a DIFFERENT origin"
else
  fail "a foreign-pinned installer was accepted: $out"
fi

reset_origins
write_uninstaller "$RC_DIR" "$PROD"
if fdp_compare "$PROD" "$RC" >/dev/null 2>&1; then
  fail "a foreign-pinned uninstaller was accepted"
else
  pass "refuses an uninstaller pinned at a DIFFERENT origin"
fi

echo "== 8. a gated origin with no service token is named, not mis-diagnosed =="

# Without the token every fetch of the real RC origin gets the 302 login page,
# which sniffs as "not shell" at every route: a total divergence report whose
# real cause is a missing credential. Checked against the REAL default URL,
# since that is what the https://rc.* gate keys on; nothing is fetched, because
# the refusal comes first.
out="$( (unset RC_ACCESS_CLIENT_ID RC_ACCESS_CLIENT_SECRET; fdp_compare "$PROD" "$FDP_RC_URL_DEFAULT") 2>&1 )"; rc=$?
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q 'Cloudflare Access'; then
  pass "refuses a gated origin with no service token, naming the credential"
else
  fail "a missing service token was not diagnosed (rc=$rc): $out"
fi

echo "== 9. the scrape matches the REAL install.sh and uninstall.sh =="

# Every case above feeds the derivation a FIXTURE. That proves the semantics and
# nothing about whether the scrape patterns still match the scripts this repo
# actually ships. They can rot silently: the `fewer than 2 libs` floor catches a
# scrape that finds NOTHING, but a partial miss (one declaration site reworded,
# the other still matching) sails straight through and the guard quietly stops
# checking a route that a piped run really fetches.
#
# So build an origin out of the tree's own install.sh + uninstall.sh, with their
# baked URLs rewritten to point at it, which is exactly what the site publisher
# does at deploy time. This is the ONE place the tree is read, and it is the
# test reading it, never the harness.
REAL_DIR="$ROOT/real"; REAL="file://$REAL_DIR"
mkdir -p "$REAL_DIR"
repin() { # <src> <dst>
  sed -e "s|^LUCIDOS_INSTALL_URL=.*|LUCIDOS_INSTALL_URL=\"\${LUCIDOS_INSTALL_URL:-$REAL/install.sh}\"|" \
      -e "s|^LUCIDOS_UNINSTALL_SELF_URL=.*|LUCIDOS_UNINSTALL_SELF_URL=\"\${LUCIDOS_UNINSTALL_SELF_URL:-$REAL/uninstall.sh}\"|" \
      "$1" > "$2"
}
repin "$SCRIPT_DIR/../../install.sh" "$REAL_DIR/install.sh"
repin "$SCRIPT_DIR/../../uninstall.sh" "$REAL_DIR/uninstall.sh"

real_routes="$(fdp_derive_routes "$REAL" "$(mktemp -d)" 2>&1)"; rc=$?
# The route set as of 2026-07-31. This list is an ASSERTION, not configuration:
# it must never be edited to make a red go away. A diff here means a piped run
# now fetches a different set of paths, which is the moment to check that BOTH
# of the site publisher's route-discovery paths know about the change.
real_expected="/install.sh
/uninstall.sh
/scripts/lib/headless_tarball.sh
/scripts/lib/install_common.sh
/scripts/lib/service.sh
/scripts/lib/stage_runtime.sh"
if [ "$rc" -eq 0 ] && [ "$(printf '%s\n' "$real_routes" | sort)" = "$(printf '%s\n' "$real_expected" | sort)" ]; then
  pass "the real installers derive exactly the six known publish routes"
else
  fail "the real route set changed, or the scrape stopped matching (rc=$rc): $(printf '%s' "$real_routes" | tr '\n' ' ')
        If a route was ADDED or REMOVED on purpose, update real_expected here AND make sure the site
        publisher's production and release-candidate route discovery both learned about it. If the set
        looks short, a declaration site in install.sh or uninstall.sh was reworded and fdp_derive_libs
        needs the new shape."
fi

echo "== 9b. the Access service token reaches ONLY the gated origin =="

# This job is the only one that fetches TWO origins, and the workflow sets both
# secrets for the whole job, so a header array built unconditionally would send
# the RC service token to the PUBLIC production origin on every run. `curl -L`
# forwards custom headers across a cross-host redirect too, so the exposure
# would not even be bounded by the origin we typed.
if fdp_url_is_gated "https://rc.lucidos.dev/install.sh"; then
  pass "the rc.* host is recognised as gated"
else
  fail "the real RC origin was not recognised as gated"
fi
if fdp_url_is_gated "https://lucidos.dev/install.sh"; then
  fail "production was treated as gated"
else
  pass "production is not gated"
fi
# A PATH under production is not the gated host, and must not be confused for
# it: the RC front door is its own host, not a /rc prefix on the apex.
if fdp_url_is_gated "https://lucidos.dev/rc/install.sh"; then
  fail "a /rc PATH on the apex was treated as the gated host"
else
  pass "a /rc path on production is not the gated host"
fi

# Assert the WIRING, not just the predicate: shadow curl with a shim that
# records its arguments, then drive fdp_fetch at both origins with credentials
# set. A shell function shadows the external command inside this subshell only.
hdr_probe() { # <url> -> the recorded argv
  (
    # shellcheck disable=SC2030 # confining the credentials to this subshell is the point
    export RC_ACCESS_CLIENT_ID=test-id RC_ACCESS_CLIENT_SECRET=test-secret
    curl() { printf '%s\n' "$*" > "$ROOT/curl-argv"; printf '200'; }
    fdp_fetch "$1" "$ROOT/discard" >/dev/null
    cat "$ROOT/curl-argv"
  )
}
if printf '%s' "$(hdr_probe https://rc.lucidos.dev/install.sh)" | grep -q 'CF-Access-Client-Id'; then
  pass "fdp_fetch sends the service token to the gated origin"
else
  fail "the gated origin got no service token, so every route would sniff as the login page"
fi
if printf '%s' "$(hdr_probe https://lucidos.dev/install.sh)" | grep -q 'CF-Access'; then
  fail "fdp_fetch leaked the RC service token to the production origin"
else
  pass "fdp_fetch sends NO service token to production"
fi

echo "== 10. the RC origin has one value, not two that can drift =="

# The release flow arms the RC front door through `rc_front_door_url` in
# release_rc_front_door.sh, and the harness sniffs that same origin through
# FDP_RC_URL_DEFAULT. Two constants for one URL is precisely the drift
# shape this guard exists to catch, so pin them equal rather than trusting them.
# Sourced in a subshell: that lib defines helpers of its own and none of them
# should leak into the cases above.
rc_url_from_release_lib="$( . "$SCRIPT_DIR/release_rc_front_door.sh" && rc_front_door_url )"
if [ "$rc_url_from_release_lib" = "$FDP_RC_URL_DEFAULT" ]; then
  pass "FDP_RC_URL_DEFAULT matches rc_front_door_url ($FDP_RC_URL_DEFAULT)"
else
  fail "the RC origin has drifted: rc_front_door_url says '$rc_url_from_release_lib', FDP_RC_URL_DEFAULT says '$FDP_RC_URL_DEFAULT'"
fi

echo ""
echo "  $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
