#!/usr/bin/env bash
# release_rc_front_door.sh — publish the release candidate's OWN installer to
# https://rc.lucidos.dev, and wait until that origin actually serves it, BEFORE
# the rc/<version> branch is pushed.
#
# WHY THE ORDER MATTERS (this file exists for exactly one reason)
#
# The front-door gate for a release candidate runs in CI on `push: rc/**`, in
# payload mode, against https://rc.lucidos.dev. Its rung-1 check parses
# LUCIDOS_DEFAULT_VERSION out of the installer that origin serves and FAILS
# unless it matches rc/<version> — deliberately, so the gate can never pass by
# verifying the previous candidate.
#
# But the RC origin is not published by CI. It is published from this Mac, by the Site
# Publisher trigger chain. So if the branch goes up first, GitHub starts the job
# against an origin that still holds the LAST RC (or nothing at all) and the leg
# reds for a pure ordering reason — a red that says nothing about the candidate.
#
# Hence: publish the RC, prove it is live, THEN push the branch. By the time the
# push event reaches GitHub the origin already serves this candidate, so the
# gate tests what it claims to test. This is the same "publish before you check"
# rule the production front door follows (see the verify-front-door-after-publish
# trigger); the RC leg just gets its ordering from here instead of from an event.
#
# BEST-EFFORT BY DESIGN. Every failure path in here returns non-zero WITHOUT
# calling fail(). A release must not die because the site publisher was slow or
# the engine was briefly unreachable — the caller reports that the RC front door
# is not armed, and the source-install and DMG-verify legs still gate the RC.
#
# ONE MACHINE IS NOT THE WORLD, AND THIS FILE CANNOT FIX THAT
#
# The wait runs on the maintainer's Mac, so it reads ONE Cloudflare POP. A
# runner resolves to another. On v0.18.3 this wait reported "serves 0.18.3 after
# 15s" while the runners still read 0.18.2, reddening all three front-door legs
# for a pure propagation reason.
#
# What it can do, and now does, is stop taking one read as a verdict. Every arm
# needs TWO vantages to agree: the plain edge read, and a cache-defeating read
# that reports what the origin holds. That separates "the publisher has not
# deployed" from "it deployed and propagation is running", and it records the
# POP that answered so a red runner leg can be compared against it.
#
# What it cannot do is reach a DIFFERENT POP. Cloudflare anycast offers no route
# to one, and a third-party fetch service is not a dependency a release gate
# should take. The bounded retry on CI's rung 1 covers a lagging remote POP;
# that is the load-bearing half of the fix. See .claude/rules/front-door.md.

# rc_front_door_url — the origin the RC front door is served from.
rc_front_door_url() { printf 'https://rc.lucidos.dev'; }

# The RC origin is gated by Cloudflare Access, so every fetch of it must present
# the `lucidos-ci-front-door` service token. CI gets it from repo secrets; this
# script gets it from the environment, optionally sourced from a local env file
# so the operator does not have to export it by hand every release.
#
# Fails SOFT when absent: without credentials the gate answers 302 to the login
# page, rc_front_door_serves_version sees HTML and returns 1, and the arm step
# reports "not serving this candidate" rather than exploding. That is the same
# non-fatal shape as every other failure in this file.
RC_ACCESS_ENV_FILE="${RC_ACCESS_ENV_FILE:-$HOME/.config/lucidos/rc-access.env}"

rc_access_headers() {
    if [[ -z "${RC_ACCESS_CLIENT_ID:-}" && -f "$RC_ACCESS_ENV_FILE" ]]; then
        # shellcheck disable=SC1090
        . "$RC_ACCESS_ENV_FILE"
    fi
    [[ -n "${RC_ACCESS_CLIENT_ID:-}" && -n "${RC_ACCESS_CLIENT_SECRET:-}" ]] || return 1
    printf '%s\n%s\n' \
        "CF-Access-Client-Id: ${RC_ACCESS_CLIENT_ID}" \
        "CF-Access-Client-Secret: ${RC_ACCESS_CLIENT_SECRET}"
}

# rc_front_door_request_publish <rc-tree> <version>
#
# Emit SitePublishRequested carrying `rc_dir`, which is what tells the publisher
# to add the /rc/* routes to the SAME deploy as production. (A Pages deployment
# replaces the entire manifest, so the RC routes must ride along with the real
# front door or publishing one would take the other down.)
#
# The path travels in the EVENT rather than in an env var because the publisher
# runs as an engine-spawned trigger: release.sh cannot set a variable in its
# environment. `lucidos` is on PATH in every Lucidos-spawned subprocess; when it
# is absent (a manual run from a plain shell) this is a no-op that returns 1, and
# the caller degrades to pushing without an armed RC front door.
rc_front_door_request_publish() {
    local rc_tree="$1" version="$2"

    if ! command -v lucidos >/dev/null 2>&1; then
        echo "    NOTE: 'lucidos' not on PATH — cannot ask Site Publisher to publish"
        echo "          the RC front door. Run the release from a Lucidos thread, or"
        echo "          publish manually, to arm the front-door payload leg."
        return 1
    fi

    [[ -d "$rc_tree" ]] || {
        echo "    WARNING: RC tree '$rc_tree' is not a directory — not requesting a publish." >&2
        return 1
    }

    # Payload literals only: a plain path, an N.N.N version, a fixed reason.
    if ! lucidos events emit SitePublishRequested \
        --summary "Publish the v${version} release candidate's front door at $(rc_front_door_url) before pushing rc/${version}" \
        --payload "{\"site\":\"lucidos.dev\",\"reason\":\"rc/${version} front door\",\"rc_dir\":\"${rc_tree}\",\"rc_version\":\"${version}\"}" \
        >/dev/null 2>&1; then
        echo "    WARNING: could not emit SitePublishRequested — the RC front door will not be armed." >&2
        return 1
    fi
    return 0
}

# What the last read saw. Globals rather than stdout, because a `$( )` capture
# runs in a subshell and anything recorded there would not survive it.
RC_FRONT_DOOR_SERVED_VERSION=""   # the baked version, empty when unreadable
RC_FRONT_DOOR_EDGE_VERSION=""     # ditto, from the plain read
RC_FRONT_DOOR_ORIGIN_VERSION=""   # ditto, from the cache-defeating read
RC_FRONT_DOOR_LAST_POP=""         # the Cloudflare POP that answered
RC_FRONT_DOOR_NONCE=0             # bumped per cache-defeating fetch

# _rc_front_door_read <url> <vantage>
#
# Read the served installer once and record what it said in the globals above.
# Returns 0 only when a version was parsed.
#
# `vantage` picks WHICH copy is read:
#
#   edge   - the plain URL, which is what a stranger on this POP gets.
#   origin - the same path with the edge cache defeated, which is what
#            Cloudflare holds. Its cache key includes the query string, so a
#            per-attempt nonce forces a MISS.
#
# A `file://` fixture has no edge cache and cannot carry a query string, so it
# is always read plainly. The distinction is meaningless there.
#
# Fails closed on every ambiguity: a curl error, an empty body, an unparsable
# installer, or a soft-404 HTML page (Cloudflare Pages answers a missing path
# with the landing page at status 200, so the STATUS proves nothing and only the
# payload does — this is the exact failure that started all of this).
_rc_front_door_read() {
    # Two `local`s, not one: a right-hand side is expanded before `local` runs,
    # so `fetch_url="$url/…"` on the same line reads the CALLER's `url` under
    # bash's dynamic scoping rather than the parameter just bound here.
    local url="$1" vantage="$2" body hdr_file
    local fetch_url="$url/install.sh"
    RC_FRONT_DOOR_SERVED_VERSION=""

    local -a hdr=()
    local line
    while IFS= read -r line; do
        [[ -n "$line" ]] && hdr+=(-H "$line")
    done < <(rc_access_headers || true)

    if [[ "$vantage" == origin && "$url" != file://* ]]; then
        RC_FRONT_DOOR_NONCE=$(( RC_FRONT_DOOR_NONCE + 1 ))
        fetch_url="$url/install.sh?cb=$(date +%s)-${RC_FRONT_DOOR_NONCE}"
        hdr+=(-H "Cache-Control: no-cache" -H "Pragma: no-cache")
    fi

    hdr_file="$(mktemp)"
    # ${hdr[@]+"${hdr[@]}"} rather than "${hdr[@]}": macOS ships bash 3.2, where
    # expanding an EMPTY array under `set -u` is an unbound-variable abort. That
    # made the no-credentials path die instead of returning 1 — which still
    # looked like a pass, because both are non-zero.
    body="$(curl -fsSL --max-time 20 -D "$hdr_file" ${hdr[@]+"${hdr[@]}"} "$fetch_url" 2>/dev/null)" || {
        rm -f "$hdr_file"
        return 1
    }
    # `cf-ray` is `<id>-<COLO>`, and that trailing token is the answering POP's
    # airport code. `-L` writes one header block per redirect hop, so take the
    # last. An origin that sends no cf-ray (a fixture) simply leaves it empty.
    RC_FRONT_DOOR_LAST_POP="$(tr -d '\r' < "$hdr_file" \
        | sed -n 's/^[Cc][Ff]-[Rr][Aa][Yy]:[[:space:]]*[^-]*-\([A-Za-z]*\).*/\1/p' | tail -n 1)"
    rm -f "$hdr_file"

    [[ -n "$body" ]] || return 1

    # HTML means the route is not there yet, whatever the status said.
    case "$(printf '%s' "$body" | sed -e '/^[[:space:]]*$/d' -e 's/^[[:space:]]*//' | head -n 1)" in
        '<'*) return 1 ;;
    esac

    RC_FRONT_DOOR_SERVED_VERSION="$(printf '%s' "$body" \
        | sed -n 's/^LUCIDOS_DEFAULT_VERSION="\([^"]*\)".*/\1/p' | head -n 1)"
    [[ -n "$RC_FRONT_DOOR_SERVED_VERSION" ]]
}

# rc_front_door_serves_version <version> [url] [vantage]
#
# True when the RC origin serves an installer that BAKES <version> from one
# vantage (the edge by default). This is the same assertion CI's rung 1 makes,
# checked here first so the operator finds out in the terminal instead of in a
# red workflow run.
rc_front_door_serves_version() {
    local version="$1" url="${2:-$(rc_front_door_url)}" vantage="${3:-edge}"
    _rc_front_door_read "$url" "$vantage" || return 1
    [[ "$RC_FRONT_DOOR_SERVED_VERSION" == "$version" ]]
}

# rc_front_door_confirms_version <version> [url]
#
# True when BOTH vantages this machine can reach serve <version>. Neither alone
# arms the gate, and the pair separates the two reasons an arm is premature.
#
# A stale edge over a fresh origin means the deploy landed and propagation is
# still running, so other POPs may be behind too. A stale origin means the
# publisher has not deployed this candidate at all.
#
# It does NOT prove that another POP agrees, and no version of this file can.
# Cloudflare anycast leaves the release machine no route to a different POP, and
# a third-party fetch service is not a dependency a release gate should take.
# CI's rung 1 carries a bounded retry for a lagging remote POP; that is the
# load-bearing half. See .claude/rules/front-door.md.
rc_front_door_confirms_version() {
    local version="$1" url="${2:-$(rc_front_door_url)}"
    RC_FRONT_DOOR_EDGE_VERSION=""
    RC_FRONT_DOOR_ORIGIN_VERSION=""

    if _rc_front_door_read "$url" edge; then
        RC_FRONT_DOOR_EDGE_VERSION="$RC_FRONT_DOOR_SERVED_VERSION"
    fi
    if _rc_front_door_read "$url" origin; then
        RC_FRONT_DOOR_ORIGIN_VERSION="$RC_FRONT_DOOR_SERVED_VERSION"
    fi

    [[ "$RC_FRONT_DOOR_EDGE_VERSION" == "$version" ]] \
        && [[ "$RC_FRONT_DOOR_ORIGIN_VERSION" == "$version" ]]
}

# One line naming what each vantage last served, and which POP answered. Every
# message about a premature arm quotes it, so a red CI leg can be compared
# against what this Mac actually saw.
rc_front_door_reading() {
    printf 'edge=%s origin=%s POP=%s' \
        "${RC_FRONT_DOOR_EDGE_VERSION:-unreadable}" \
        "${RC_FRONT_DOOR_ORIGIN_VERSION:-unreadable}" \
        "${RC_FRONT_DOOR_LAST_POP:-unknown}"
}

# rc_front_door_wait <version> [timeout-secs] [url]
#
# Poll until BOTH vantages serve <version>. Returns 0 as soon as they do, 1 on
# timeout. The publish is a Cloudflare deploy plus the publisher's own
# verification, so tens of seconds is normal and a couple of minutes is not
# alarming; the default ceiling is generous because the cost of waiting is a
# little operator time and the cost of not waiting is a meaningless red gate.
rc_front_door_wait() {
    local version="$1" timeout="${2:-300}" url="${3:-$(rc_front_door_url)}"
    local waited=0 interval=5

    while (( waited < timeout )); do
        if rc_front_door_confirms_version "$version" "$url"; then
            echo "    $url/install.sh serves $version after ${waited}s, at the edge and at the origin ($(rc_front_door_reading))"
            return 0
        fi
        sleep "$interval"
        waited=$(( waited + interval ))
        (( waited % 30 == 0 )) && echo "    still waiting for $url/install.sh to serve $version (${waited}s, $(rc_front_door_reading))"
    done

    echo "    TIMED OUT after ${timeout}s waiting for $url/install.sh to serve $version" >&2
    echo "    Last read: $(rc_front_door_reading). A stale origin means the publisher" >&2
    echo "    has not deployed this candidate; a stale edge means it has, and this" >&2
    echo "    POP is still catching up." >&2
    return 1
}

# rc_front_door_arm <rc-tree> <version> [timeout-secs] [url]
#
# The whole sequence the caller wants: request the publish, wait for the origin,
# report. Returns 0 when the RC front door is live and serving this candidate,
# 1 otherwise — never fatal. The caller decides what to say; nothing here aborts
# a release.
#
# The origin is a parameter (defaulting to the real one) rather than a constant
# read inside: a function that reaches for the production URL internally cannot
# be pointed at a fixture, so its most important branch — the short-circuit —
# would be untestable offline.
rc_front_door_arm() {
    local rc_tree="$1" version="$2" timeout="${3:-300}" url="${4:-$(rc_front_door_url)}"

    # Already live (a re-run, or a publish that happened by other means): don't
    # spend a deploy on it. Makes --push-rc retries cheap and idempotent. Held
    # to the same two-vantage bar as the wait below, since a short-circuit is an
    # arm too: an edge-only read here is exactly the sample-for-a-verdict
    # mistake the wait exists to avoid.
    if rc_front_door_confirms_version "$version" "$url"; then
        echo "    $url/install.sh already serves $version ($(rc_front_door_reading))"
        return 0
    fi

    rc_front_door_request_publish "$rc_tree" "$version" || return 1
    echo "    asked Site Publisher to publish the RC front door from $rc_tree"
    rc_front_door_wait "$version" "$timeout" "$url"
}
