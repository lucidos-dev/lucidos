#!/usr/bin/env bash
# front_door_parity.sh: assert the PRODUCTION and RELEASE-CANDIDATE front doors
# serve the SAME set of publish routes, and name every route where they differ.
#
#   ./scripts/lib/front_door_parity.sh                       # the two real origins
#   ./scripts/lib/front_door_parity.sh <prod-url> <rc-url>   # any pair (tests use file://)
#
# ── WHY THIS EXISTS ──────────────────────────────────────────────────────────
#
# A *publish route* is any path a piped run would curl back from its own origin
# and execute. `install.sh` and `uninstall.sh`, when piped, have no checkout:
# they fetch their helper libs and each other from the origin they were served
# from and run the result. Cloudflare Pages answers an unknown path with the SPA
# fallback (the landing page) at status 200, so a missing route is not a 404, it
# is "pipe a web page into sh". That is the 2026-07-29 clean-machine failure the
# whole front-door gate exists to prevent.
#
# The rule "if a piped run would curl it back from its own origin, it is a
# publish route" is enforced independently in TWO places in the site publisher:
# one route-discovery path for production, a separate one for the release
# candidate. They can drift, and on 2026-07-30 they did. `/uninstall.sh` was
# added to the production path only; the omission surfaced on the rc/0.18.0 push
# the next morning as three red `front-door (rc, payload only)` legs.
#
# Every front-door job sees exactly ONE origin, so none of them can observe that
# the other origin serves a different set. This script is the only thing in the
# repo that looks at both, which is why the divergence check lives here.
#
# ── WHAT IT READS, AND WHAT IT DELIBERATELY DOES NOT ─────────────────────────
#
# The route set for an origin is derived from THAT ORIGIN'S OWN served
# `install.sh` and `uninstall.sh`, exactly as the CI rung-1 sniff does. It is
# never read from the tree this script lives in and never hardcoded, so a route
# added to the installers is covered the day it ships rather than the day
# somebody remembers to update a list here.
#
# That distinction is load-bearing when this runs in CI. The `front-door` jobs
# take no checkout at all, so that they can know nothing about the tree they
# live in. This script IS checked out, because it is the measuring instrument
# rather than the subject: nothing it reports comes from the local tree.
#
# ── SEVERITY IS ASYMMETRIC, ON PURPOSE ───────────────────────────────────────
#
#   production serves it, the RC does not  -> FATAL. The 2026-07-30 shape. The
#       RC front door has fallen behind, the next rc/** push will red on it, and
#       it is actionable now.
#   the RC serves it, production does not  -> WARNING. An in-flight candidate
#       legitimately adds routes before production has them. Failing here would
#       red the daily cron through every such release window. It does not go
#       unguarded: once the GA publishes, production's own served installer
#       declares the route and the `front-door` job's rung 1 reds fatally on it.
#   neither serves it                      -> WARNING. Both front doors are
#       broken, which is the `front-door` job's verdict to give (it runs in the
#       same daily workflow run). Reporting it fatally here would only duplicate
#       a red.
#
# ── FAIL CLOSED ──────────────────────────────────────────────────────────────
#
# Every parse and every precondition exits non-zero rather than reporting
# parity: an origin serving no installer, an installer that is HTML, a lib
# declaration that scrapes to nothing, an unparsable or foreign-pinned
# `LUCIDOS_INSTALL_URL`. A comparison that could not be made must never read as
# "the two origins agree".
#
# Offline-tested by scripts/lib/front_door_parity_test.sh (file:// origins).

# ── the two real origins ─────────────────────────────────────────────────────
# Defaults only. Both are overridable by argument so the test can point the
# whole script at a file:// tree.
#
# The RC origin has a second definition, `rc_front_door_url` in
# release_rc_front_door.sh, which the release flow uses to arm the front door.
# Two constants for one URL is the drift shape this whole script exists to
# catch, so front_door_parity_test.sh asserts the two are equal rather than
# trusting them to stay that way. They are not merged because the release lib
# reaches for the `lucidos` CLI and a CI-side sniffer must not depend on it.
FDP_PROD_URL_DEFAULT="https://lucidos.dev"
FDP_RC_URL_DEFAULT="https://rc.lucidos.dev"

# fdp_url_is_gated <url>: true when <url> is on an origin behind Cloudflare
# Access, which today is the rc.* host and nothing else.
#
# ONE definition, used by both the credential precheck and the fetch, precisely
# so the question "does this request carry the service token?" cannot be
# answered differently in two places. Matched on the `https://rc.` HOST prefix,
# so a path like https://lucidos.dev/rc/install.sh is correctly NOT gated.
fdp_url_is_gated() {
    case "$1" in
        https://rc.*) return 0 ;;
        *) return 1 ;;
    esac
}

# fdp_access_headers: the Cloudflare Access service token, one "Header: value"
# per line, or nothing when it is not configured.
#
# This job is unusual in fetching TWO origins, only one of which is gated, so
# the token must be attached PER REQUEST rather than per run. Callers must go
# through fdp_fetch, which asks fdp_url_is_gated first: sending an RC service
# token to the public production origin would disclose the credential to a host
# that has no use for it, and `curl -L` forwards custom headers across a
# cross-host redirect, so the exposure is not bounded by the origin we typed.
fdp_access_headers() {
    [ -n "${RC_ACCESS_CLIENT_ID:-}" ] && [ -n "${RC_ACCESS_CLIENT_SECRET:-}" ] || return 1
    printf '%s\n%s\n' \
        "CF-Access-Client-Id: ${RC_ACCESS_CLIENT_ID}" \
        "CF-Access-Client-Secret: ${RC_ACCESS_CLIENT_SECRET}"
}

# fdp_require_access <url>: refuse to run against a GATED origin with no
# credentials, naming the cause. A missing secret and a dead origin look
# identical in a payload sniff, and only one of them is a route regression.
#
# Takes any origin and no-ops for an ungated one, so fdp_compare can ask it
# about BOTH. Naming it for the rc origin specifically would bake in the
# assumption that production can never be gated, which is not ours to make.
fdp_require_access() {
    local url="$1"
    fdp_url_is_gated "$url" || return 0
    if fdp_access_headers >/dev/null; then
        return 0
    fi
    echo "ERROR: $url is behind Cloudflare Access but RC_ACCESS_CLIENT_ID / RC_ACCESS_CLIENT_SECRET are not set." >&2
    echo "       Every fetch would get the 302 login page and be reported as 'not shell', which reads as a" >&2
    echo "       total route divergence when the real cause is a missing credential. Set both (Cloudflare ->" >&2
    echo "       Access -> Service Tokens -> lucidos-ci-front-door) and re-run." >&2
    return 1
}

# fdp_fetch <url> <outfile>: GET into <outfile>, echo the HTTP status.
#
# Deliberately NOT `curl -f`: a hard 4xx and a soft 404 are different diagnoses
# and both matter here, so the status is captured rather than turned into an
# exit code. `file://` reports 000 with a zero exit, which is how the offline
# test drives the real fetch path.
#
# The Access token is attached ONLY to a gated URL. Building the array
# unconditionally would send the RC service token to production on every run,
# since the workflow sets both secrets for the whole job.
fdp_fetch() {
    local url="$1" out="$2" code
    local -a hdr=()
    local line
    if fdp_url_is_gated "$url"; then
        while IFS= read -r line; do
            [ -n "$line" ] && hdr+=(-H "$line")
        done < <(fdp_access_headers || true)
    fi

    : > "$out"
    # ${hdr[@]+"${hdr[@]}"} rather than "${hdr[@]}": macOS ships bash 3.2, where
    # expanding an EMPTY array under `set -u` is an unbound-variable abort. This
    # script runs on a Linux runner today and by hand on a Mac tomorrow.
    code="$(curl -sSL --max-time 30 -o "$out" -w '%{http_code}' \
        ${hdr[@]+"${hdr[@]}"} "$url" 2>/dev/null)" || code="000"
    printf '%s' "$code"
}

# fdp_classify <file> <http-status>: what the origin actually served.
#
#   shell        a `#!` shebang on line 1, which is the only acceptable answer
#   html         a SOFT 404: the landing page at status 200, the dangerous case,
#                because a piped run EXECUTES it
#   empty        a zero-byte body at status 200
#   http-<code>  an honest error status
#   not-shell    something else entirely is at this URL, at status 200
#
# The status is what separates `html` from `http-404`, and the distinction is
# worth keeping: production soft-404s (Pages SPA fallback at 200) while the RC
# project answers a hard error, so the same missing route reports differently at
# the two origins and the reader should be able to see which they are looking
# at. Both are "not shell", so neither can pass.
#
# Leading blank lines and indentation are stripped before the sniff: a
# prettified SPA fallback must not sneak past a naive first-byte check.
fdp_classify() {
    local f="$1" code="$2" first

    if [ ! -s "$f" ]; then
        case "$code" in
            200|000) printf 'empty' ;;
            *) printf 'http-%s' "$code" ;;
        esac
        return 0
    fi

    first="$(sed -e '/^[[:space:]]*$/d' -e 's/^[[:space:]]*//' "$f" 2>/dev/null | head -n 1)"
    case "$first" in
        '#!'*) printf 'shell'; return 0 ;;
    esac
    case "$code" in
        200|000)
            case "$first" in
                '<'*) printf 'html' ;;
                *) printf 'not-shell' ;;
            esac ;;
        *) printf 'http-%s' "$code" ;;
    esac
}

# fdp_probe <origin> <path>: classify one route at one origin.
fdp_probe() {
    local origin="$1" path="$2" tmp code
    tmp="$(mktemp)"
    code="$(fdp_fetch "$origin$path" "$tmp")"
    fdp_classify "$tmp" "$code"
    rm -f "$tmp"
}

# fdp_derive_libs <served-script>: the helper-lib file names a piped run of this
# script would fetch, one per line.
#
# Scraped off the declaration sites rather than pinned here, so the set stays
# correct when it changes. Three shapes are read, covering both scripts:
#
#   ^LUCIDOS_LIBS=   install.sh's download-path list
#   _source_libs     install.sh's launch/register path (source_service_lib)
#   $base/           the literal fetch inside uninstall.sh's own
#                    source_service_lib, which install.sh does NOT declare.
#                    Reading it here is what keeps the uninstaller's helper from
#                    being covered only by coincidence (its one lib, service.sh,
#                    happens to be in install.sh's list today).
#
# `install.sh` and `uninstall.sh` are filtered out: they are top-level routes in
# their own right, and both appear as `.sh` tokens on lines that also carry a
# lib-base expression.
fdp_derive_libs() {
    grep -E '^LUCIDOS_LIBS=|_source_libs |\$\{?base\}?/' "$1" 2>/dev/null \
        | grep -oE '[A-Za-z0-9_]+\.sh' \
        | grep -vxE 'install\.sh|uninstall\.sh' \
        | sort -u
}

# fdp_parse_pinned_url <served-script> <var-name>: the URL a served script bakes
# into <var-name>, resolved out of its `${VAR:-<default>}` assignment. Echoes
# nothing when it cannot be parsed, which every caller treats as fatal.
fdp_parse_pinned_url() {
    grep -m1 "^$2=" "$1" 2>/dev/null | sed -e 's/.*:-//' -e 's/}"$//' -e 's/"$//'
}

# fdp_derive_routes <origin> <workdir>: the publish routes a piped run FROM THIS
# ORIGIN would fetch, one path per line, or non-zero with a diagnosis.
#
# Fetches the origin's own install.sh + uninstall.sh into <workdir> and reads
# the route set out of them. Every step fails closed.
fdp_derive_routes() {
    local origin="$1" work="$2"
    local code served install_url u_install_url lib_base expected libs count name

    code="$(fdp_fetch "$origin/install.sh" "$work/install.sh")"
    served="$(fdp_classify "$work/install.sh" "$code")"
    case "$served" in
        shell) ;;
        *)
            echo "ERROR: $origin/install.sh does not serve a shell script (got $served, HTTP $code)." >&2
            echo "       The route set for this origin cannot be derived, so no parity claim can be made about it." >&2
            echo "       For the RC origin this usually means the front door has never been published, or was reset:" >&2
            echo "       re-arm it with 'release.sh --push-rc <version>' (rc_front_door_arm) before reading anything into this." >&2
            return 1 ;;
    esac

    # The lib base a piped run would really hit, resolved the same way
    # _source_libs resolves it. EQUALITY against this origin, never a prefix
    # match: with two origins under one apex a prefix match is vacuous, and an
    # installer pinned at the other origin would make every path probed below
    # the wrong path.
    install_url="$(fdp_parse_pinned_url "$work/install.sh" LUCIDOS_INSTALL_URL)"
    case "$install_url" in
        http*://*/install.sh|file://*/install.sh) ;;
        *)
            echo "ERROR: could not parse LUCIDOS_INSTALL_URL out of $origin/install.sh (got '$install_url')." >&2
            echo "       This must never silently pass, so it fails closed." >&2
            return 1 ;;
    esac
    lib_base="${install_url%/install.sh}/scripts/lib"
    expected="$origin/scripts/lib"
    if [ "$lib_base" != "$expected" ]; then
        echo "ERROR: $origin/install.sh points its helper-lib fetch at $lib_base, not $expected." >&2
        echo "       A piped install from this origin would source its libs from somewhere else, so comparing" >&2
        echo "       this origin's script paths against the other's would compare the wrong things." >&2
        return 1
    fi

    libs="$(fdp_derive_libs "$work/install.sh")"
    count="$(printf '%s\n' "$libs" | grep -c . || true)"
    if [ "${count:-0}" -lt 2 ]; then
        echo "ERROR: scraped only ${count:-0} helper lib name(s) from $origin/install.sh." >&2
        echo "       Its lib declaration changed shape and this scrape needs updating; it must never check nothing." >&2
        return 1
    fi

    # The uninstaller is a publish route AND a second script with fetches of its
    # own, so its lib declarations join the set. A soft 404 here is reported by
    # the comparison rather than fatally: "the uninstaller is missing at one
    # origin" is precisely the divergence this script exists to name, and dying
    # here would report it as an unrelated derivation failure.
    code="$(fdp_fetch "$origin/uninstall.sh" "$work/uninstall.sh")"
    if [ "$(fdp_classify "$work/uninstall.sh" "$code")" = "shell" ]; then
        u_install_url="$(fdp_parse_pinned_url "$work/uninstall.sh" LUCIDOS_INSTALL_URL)"
        case "$u_install_url" in
            http*://*/install.sh|file://*/install.sh)
                if [ "${u_install_url%/install.sh}/scripts/lib" != "$expected" ]; then
                    echo "ERROR: $origin/uninstall.sh points its service.sh fetch at ${u_install_url%/install.sh}/scripts/lib, not $expected." >&2
                    return 1
                fi ;;
            *)
                echo "ERROR: could not parse LUCIDOS_INSTALL_URL out of $origin/uninstall.sh (got '$u_install_url')." >&2
                echo "       This must never silently pass, so it fails closed." >&2
                return 1 ;;
        esac
        libs="$(printf '%s\n%s\n' "$libs" "$(fdp_derive_libs "$work/uninstall.sh")" | grep . | sort -u)"
    fi

    printf '/install.sh\n/uninstall.sh\n'
    for name in $libs; do
        printf '/scripts/lib/%s\n' "$name"
    done
}

# fdp_compare <prod-url> <rc-url>: the whole check. Prints the route table and
# the verdict; returns 0 when the two front doors agree (warnings allowed) and 1
# on a divergence or on any failure to derive.
fdp_compare() {
    # Strip EVERY trailing slash, not one: "<origin>//install.sh" would soft-404
    # into a confusing HTML diagnosis, and the lib-base EQUALITY check below
    # would fail against a base that is really correct. Same normalisation the
    # workflow's own origin validation does.
    local prod="$1" rc="$2"
    while [ "${prod%/}" != "$prod" ]; do prod="${prod%/}"; done
    while [ "${rc%/}" != "$rc" ]; do rc="${rc%/}"; done
    local work prod_routes rc_routes union path sp sr
    local fatal=0 warned=0 label

    fdp_require_access "$prod" || return 1
    fdp_require_access "$rc" || return 1

    work="$(mktemp -d)"
    mkdir -p "$work/prod" "$work/rc"
    : > "$work/verdicts"

    echo "==> Deriving the publish routes each origin's OWN scripts would fetch"
    echo "    production: $prod"
    echo "    candidate:  $rc"
    echo

    prod_routes="$(fdp_derive_routes "$prod" "$work/prod")" || { rm -rf "$work"; return 1; }
    rc_routes="$(fdp_derive_routes "$rc" "$work/rc")" || { rm -rf "$work"; return 1; }

    echo "    production declares: $(printf '%s' "$prod_routes" | tr '\n' ' ')"
    echo "    candidate declares:  $(printf '%s' "$rc_routes" | tr '\n' ' ')"
    echo

    union="$(printf '%s\n%s\n' "$prod_routes" "$rc_routes" | grep . | sort -u)"

    # Probe ONCE per route and keep the verdicts, rather than re-fetching for
    # the detail pass below. Halves the requests, and more importantly means the
    # table and the advice can never disagree because an origin changed
    # mid-report. A file rather than an associative array: macOS ships bash 3.2,
    # which has none.
    echo "==> What each origin actually serves"
    printf '    %-34s %-12s %-12s %s\n' ROUTE PRODUCTION CANDIDATE VERDICT
    for path in $union; do
        sp="$(fdp_probe "$prod" "$path")"
        sr="$(fdp_probe "$rc" "$path")"
        printf '%s\t%s\t%s\n' "$path" "$sp" "$sr" >> "$work/verdicts"
        if [ "$sp" = "shell" ] && [ "$sr" = "shell" ]; then
            label="ok"
        elif [ "$sp" = "shell" ]; then
            label="DIVERGED (candidate)"
            fatal=1
        elif [ "$sr" = "shell" ]; then
            label="diverged (production)"
            warned=1
        else
            label="missing at both"
            warned=1
        fi
        printf '    %-34s %-12s %-12s %s\n' "$path" "$sp" "$sr" "$label"
    done
    echo

    # The per-route detail, after the table, so the table stays readable and the
    # advice sits next to the verdict.
    while IFS="$(printf '\t')" read -r path sp sr; do
        [ -n "$path" ] || continue
        if [ "$sp" = "shell" ] && [ "$sr" != "shell" ]; then
            echo "ERROR: $rc$path is not a shell script (it is $sr), but $prod$path is."
            echo "       The two front doors serve DIFFERENT route sets. A piped run from the candidate origin"
            echo "       would fetch $path and execute whatever came back, which on a soft 404 is the landing page."
            echo "       CAUSE: the site publisher discovers routes separately for production and for the release"
            echo "       candidate, and only the production path knows about $path. Both must be updated together;"
            echo "       that deploy runs on the maintainer's machine off the SitePublished chain, not in CI, so no"
            echo "       change in this repository can turn this green."
        elif [ "$sr" = "shell" ] && [ "$sp" != "shell" ]; then
            echo "WARNING: $prod$path is not a shell script (it is $sp), but $rc$path is."
            echo "         Expected while a candidate that ADDS this route is in flight: production gets it at"
            echo "         publish. Not expected otherwise, and it becomes a hard failure the moment the release"
            echo "         publishes, because production's own installer will then declare a route production does"
            echo "         not serve. Make sure the publisher's production route discovery knows about $path."
        elif [ "$sp" != "shell" ] && [ "$sr" != "shell" ]; then
            echo "WARNING: neither front door serves $path (production: $sp, candidate: $sr)."
            echo "         The two agree, so this is not a divergence. It is the front-door job's verdict to give,"
            echo "         and that job runs against production in this same workflow run."
        fi
    done < "$work/verdicts"

    rm -rf "$work"

    if [ "$fatal" -ne 0 ]; then
        echo
        echo "FAILED: the release-candidate front door is missing at least one route the production front door serves."
        return 1
    fi
    if [ "$warned" -ne 0 ]; then
        echo
        echo "OK (with warnings): no route is served by production and missing from the candidate."
        return 0
    fi
    echo "OK: both front doors serve the same route set, and every route is shell."
    return 0
}

# Runnable directly, sourceable by the test. Guarding on BASH_SOURCE rather than
# on a flag keeps the test free to call the helpers one at a time.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    set -uo pipefail
    fdp_compare "${1:-$FDP_PROD_URL_DEFAULT}" "${2:-$FDP_RC_URL_DEFAULT}"
fi
