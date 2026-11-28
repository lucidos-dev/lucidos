#!/usr/bin/env bash
# release_deadline.sh: the notarization deadline. Parses `--notarize-deadline
# <spec>` into an absolute instant, and names the exit code a deadline expiry
# uses.
#
# WHY IT EXISTS. `notarize_poll` in build-dmg.sh dies when NOTARIZE_POLL_TIMEOUT
# expires, which turns "Apple is slow" into a FAILED run. That is the right
# answer for a human at a keyboard (they wanted a verdict and did not get one),
# and the wrong answer for an unattended nightly, which only ever meant to
# PREPARE a release: an outstanding verdict is a pause, not a failure. With a
# deadline the poll stops at the given instant, the run exits 0 down a "notary
# pending" path, and the resume handle plus the worktree are left exactly as the
# existing --resume-notarize expects to find them.
#
# THE DEADLINE IS AN ABSOLUTE INSTANT, RESOLVED ONCE. A duration resolves against
# the moment the spec is parsed (the start of the run), never against the moment
# polling happens to begin. Two reasons: an unattended caller's real requirement
# is a wall-clock bound on the whole run ("hand back by 06:30", "never run more
# than two hours"), and a value fixed at parse time is the same value however
# many polls, resumes or stages the run passes through. release.sh resolves the
# operator's spec once and hands build-dmg.sh the already-resolved `@<epoch>`
# form, so the two cannot drift.
#
# Accepted spec forms:
#   90m / 2h / 5400s   a duration from now
#   06:30              the NEXT time it is 06:30 local
#   @1785000000        an absolute epoch (what release.sh passes down)
#
# Pure shell arithmetic plus a single `date` call in the one impure wrapper, so
# the parser is exercised offline by scripts/lib/release_deadline_test.sh. It is
# sourced by build-dmg.sh, which SHIPS, so this lib ships too: it carries nothing
# machine-specific and names no internal path. Contrast release_preflight.sh and
# release_abandon.sh, which only release.sh sources and which are withheld.

# The exit code build-dmg.sh uses for "the deadline expired, the verdict is
# outstanding, nothing failed". Deliberately not 0: the caller has to be able to
# tell a completed Phase A (staged, gate armable) from a paused one (no staging
# at all), and inferring that from what happens to be on disk is exactly the kind
# of implicit signal that rots. release.sh maps it back to `exit 0`, because at
# THAT layer the run genuinely succeeded.
#
# Not readonly: a test harness may source this file twice, and a readonly
# redefinition is a hard error there.
# shellcheck disable=SC2034 # the whole point is cross-file: build-dmg.sh exits with it, release.sh reads it back
RELEASE_NOTARY_PENDING_EXIT=20

# release_deadline_accepted_forms: the one description of what a spec may look
# like, so every refusal names the same set.
release_deadline_accepted_forms() {
    printf 'a duration (90m, 2h, 5400s), a local wall-clock time (06:30), or an absolute epoch (@1785000000)'
}

# release_deadline_resolve <spec> <now-epoch> <now-seconds-into-local-day>
#
# Print the absolute epoch the deadline falls at, or fail with a message naming
# the accepted forms. PURE: every input is a parameter, which is what makes the
# "next 06:30" arithmetic testable without waiting for tomorrow.
#
# <now-seconds-into-local-day> is the local H*3600 + M*60 + S of <now-epoch>. The
# caller computes it, because deriving it here would need a second `date` call
# with a different flag on BSD (`-r`) than on GNU (`-d @`), and a parser that
# shells out is a parser that cannot be unit-tested.
release_deadline_resolve() {
    local spec="$1" now="$2" into_day="$3"
    local value unit hh mm midnight target

    case "$spec" in
        @*)
            value="${spec#@}"
            case "$value" in
                ''|*[!0-9]*)
                    echo "ERROR: '$spec' is not an epoch. Accepted: $(release_deadline_accepted_forms)." >&2
                    return 1
                    ;;
            esac
            printf '%s' "$((10#$value))"
            return 0
            ;;
        *:*)
            # A local wall-clock time, HH:MM, meaning the NEXT time it is that.
            hh="${spec%%:*}"
            mm="${spec#*:}"
            case "$hh" in ''|*[!0-9]*) hh="x" ;; esac
            case "$mm" in ''|*[!0-9]*) mm="x" ;; esac
            if [ "$hh" = "x" ] || [ "$mm" = "x" ] || [ "${#hh}" -gt 2 ] || [ "${#mm}" -ne 2 ]; then
                echo "ERROR: '$spec' is not a wall-clock time. Accepted: $(release_deadline_accepted_forms)." >&2
                return 1
            fi
            # 10# forces base 10: `08` and `09` are invalid OCTAL, and a leading
            # zero is exactly what a wall-clock time is written with.
            hh="$((10#$hh))"
            mm="$((10#$mm))"
            if [ "$hh" -gt 23 ] || [ "$mm" -gt 59 ]; then
                echo "ERROR: '$spec' is not a valid time of day (00:00 to 23:59)." >&2
                return 1
            fi
            midnight="$((now - into_day))"
            target="$((midnight + hh * 3600 + mm * 60))"
            # "The NEXT time it is HH:MM": already past (or exactly now) means
            # tomorrow. Adding a flat 86400 shifts by an hour across a DST
            # boundary, which is accepted. An hour of slack on a deadline whose
            # whole job is "stop waiting eventually" costs nothing, and the
            # alternative is a timezone database this script has no business
            # carrying.
            [ "$target" -gt "$now" ] || target="$((target + 86400))"
            printf '%s' "$target"
            return 0
            ;;
        *[0-9]s|*[0-9]m|*[0-9]h)
            unit="${spec: -1}"
            value="${spec%?}"
            case "$value" in
                ''|*[!0-9]*)
                    echo "ERROR: '$spec' is not a duration. Accepted: $(release_deadline_accepted_forms)." >&2
                    return 1
                    ;;
            esac
            value="$((10#$value))"
            if [ "$value" -le 0 ]; then
                echo "ERROR: '$spec' is not a positive duration." >&2
                return 1
            fi
            case "$unit" in
                s) target="$((now + value))" ;;
                m) target="$((now + value * 60))" ;;
                h) target="$((now + value * 3600))" ;;
            esac
            printf '%s' "$target"
            return 0
            ;;
    esac

    echo "ERROR: '$spec' is not a notarization deadline. Accepted: $(release_deadline_accepted_forms)." >&2
    return 1
}

# release_deadline_parse <spec>: the impure wrapper. Reads the clock once and
# resolves <spec> against it. ONE `date` call, so the epoch and the
# seconds-into-day it is paired with can never straddle a second boundary.
release_deadline_parse() {
    local spec="$1" now hh mm ss
    # A single invocation printing all four fields. Splitting it into two `date`
    # calls is the classic way to resolve 23:59:59 against tomorrow's midnight.
    read -r now hh mm ss <<<"$(date '+%s %H %M %S')" || {
        echo "ERROR: could not read the local clock to resolve the deadline." >&2
        return 1
    }
    release_deadline_resolve "$spec" "$now" "$((10#$hh * 3600 + 10#$mm * 60 + 10#$ss))"
}

# release_deadline_expired <deadline-epoch> [<now-epoch>]: zero once the deadline
# has been reached. An EMPTY deadline is never expired, which is what keeps the
# no-flag path byte-identical to the old behaviour.
release_deadline_expired() {
    local deadline="$1" now="${2:-}"
    [ -n "$deadline" ] || return 1
    [ -n "$now" ] || now="$(date '+%s')"
    [ "$now" -ge "$deadline" ]
}

# release_deadline_format <epoch>: a human rendering for the handoff block. BSD
# `date` takes -r, GNU takes -d @; try both and fall back to the raw epoch rather
# than failing, because this is only ever used to print a message.
release_deadline_format() {
    local epoch="$1"
    date -r "$epoch" '+%Y-%m-%d %H:%M:%S %Z' 2>/dev/null \
        || date -d "@$epoch" '+%Y-%m-%d %H:%M:%S %Z' 2>/dev/null \
        || printf 'epoch %s' "$epoch"
}
