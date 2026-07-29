#!/usr/bin/env bash
# release_rc_front_door.sh — publish the release candidate's OWN installer to
# https://lucidos.dev/rc, and wait until that origin actually serves it, BEFORE
# the rc/<version> branch is pushed.
#
# WHY THE ORDER MATTERS (this file exists for exactly one reason)
#
# The front-door gate for a release candidate runs in CI on `push: rc/**`, in
# payload mode, against https://lucidos.dev/rc. Its rung-1 check parses
# LUCIDOS_DEFAULT_VERSION out of the installer that origin serves and FAILS
# unless it matches rc/<version> — deliberately, so the gate can never pass by
# verifying the previous candidate.
#
# But /rc is not published by CI. It is published from this Mac, by the Site
# Publisher trigger chain. So if the branch goes up first, GitHub starts the job
# against an origin that still holds the LAST RC (or nothing at all) and the leg
# reds for a pure ordering reason — a red that says nothing about the candidate.
#
# Hence: publish /rc, prove it is live, THEN push the branch. By the time the
# push event reaches GitHub the origin already serves this candidate, so the
# gate tests what it claims to test. This is the same "publish before you check"
# rule the production front door follows (see the verify-front-door-after-publish
# trigger); the RC leg just gets its ordering from here instead of from an event.
#
# BEST-EFFORT BY DESIGN. Every failure path in here returns non-zero WITHOUT
# calling fail(). A release must not die because the site publisher was slow or
# the engine was briefly unreachable — the caller reports that the RC front door
# is not armed, and the source-install and DMG-verify legs still gate the RC.

# rc_front_door_url — the origin the RC front door is served from.
rc_front_door_url() { printf 'https://lucidos.dev/rc'; }

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

# rc_front_door_serves_version <version> [url]
#
# True when the RC origin serves an installer that BAKES <version>. This is the
# same assertion CI's rung 1 makes, checked here first so the operator finds out
# in the terminal instead of in a red workflow run.
#
# Fails closed on every ambiguity: a curl error, an empty body, an unparsable
# installer, or a soft-404 HTML page (Cloudflare Pages answers a missing path
# with the landing page at status 200, so the STATUS proves nothing and only the
# payload does — this is the exact failure that started all of this).
rc_front_door_serves_version() {
    local version="$1" url="${2:-$(rc_front_door_url)}" body served

    body="$(curl -fsSL --max-time 20 "$url/install.sh" 2>/dev/null)" || return 1
    [[ -n "$body" ]] || return 1

    # HTML means the route is not there yet, whatever the status said.
    case "$(printf '%s' "$body" | sed -e '/^[[:space:]]*$/d' -e 's/^[[:space:]]*//' | head -n 1)" in
        '<'*) return 1 ;;
    esac

    served="$(printf '%s' "$body" | sed -n 's/^LUCIDOS_DEFAULT_VERSION="\([^"]*\)".*/\1/p' | head -n 1)"
    [[ -n "$served" ]] || return 1
    [[ "$served" == "$version" ]]
}

# rc_front_door_wait <version> [timeout-secs] [url]
#
# Poll until the RC origin serves <version>. Returns 0 as soon as it does, 1 on
# timeout. The publish is a Cloudflare deploy plus the publisher's own
# verification, so tens of seconds is normal and a couple of minutes is not
# alarming; the default ceiling is generous because the cost of waiting is a
# little operator time and the cost of not waiting is a meaningless red gate.
rc_front_door_wait() {
    local version="$1" timeout="${2:-300}" url="${3:-$(rc_front_door_url)}"
    local waited=0 interval=5

    while (( waited < timeout )); do
        if rc_front_door_serves_version "$version" "$url"; then
            echo "    $url/install.sh serves $version after ${waited}s"
            return 0
        fi
        sleep "$interval"
        waited=$(( waited + interval ))
        (( waited % 30 == 0 )) && echo "    still waiting for $url/install.sh to serve $version (${waited}s)"
    done

    echo "    TIMED OUT after ${timeout}s waiting for $url/install.sh to serve $version" >&2
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
    # spend a deploy on it. Makes --push-rc retries cheap and idempotent.
    if rc_front_door_serves_version "$version" "$url"; then
        echo "    $url/install.sh already serves $version — no publish needed"
        return 0
    fi

    rc_front_door_request_publish "$rc_tree" "$version" || return 1
    echo "    asked Site Publisher to publish the RC front door from $rc_tree"
    rc_front_door_wait "$version" "$timeout" "$url"
}
