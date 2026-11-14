#!/bin/bash
# launch_tree.sh: build the engine set from a pinned snapshot (ADR 0668).
#
# A primary checkout moves under a running cargo whenever an Apply, a landing
# or follow-main moves HEAD. So its engine build compiles a *launch tree*: a
# detached git worktree at target/launch-tree/, with its own target dir. Only a
# build holding the launch tree lock writes it, so cargo always sees one source
# state, and the stable path keeps cargo incremental across builds.
#
# A linked worktree builds live: only its own session moves its HEAD.
#
# Targets bash 3.2. Tested by launch_tree_test.sh.

# shellcheck source=scripts/lib/flock.sh
source "$(dirname "${BASH_SOURCE[0]}")/flock.sh"

launch_tree_dir() { # <project>
    echo "$1/target/launch-tree"
}

# The git common dir as an absolute path, or nothing.
_launch_tree_common_dir() { # <dir>
    local common
    common="$(git -C "$1" rev-parse --git-common-dir 2>/dev/null)" || return 1
    case "$common" in
        /*) ;;
        *) common="$1/$common" ;;
    esac
    (cd "$common" 2>/dev/null && pwd -P)
}

# Beside the repository, not under target/: `cargo clean` must never delete a
# held lock, since a new opener would then lock a new inode.
launch_tree_lock_path() { # <project>
    echo "$(_launch_tree_common_dir "$1")/lucidos-launch-tree.lock"
}

# True for a primary working tree, where the git dir is the common dir.
launch_tree_wanted() { # <project>
    local git_dir common
    git_dir="$(git -C "$1" rev-parse --absolute-git-dir 2>/dev/null)" || return 1
    git_dir="$(cd "$git_dir" 2>/dev/null && pwd -P)" || return 1
    common="$(_launch_tree_common_dir "$1")" || return 1
    [ -n "$common" ] && [ "$git_dir" = "$common" ]
}

# Make <tree> a registered worktree of <project>. A healthy tree is reused, so
# its target dir stays warm. A tree whose dir is gone, as after `cargo clean`,
# is re-added at the same path with --force, which reuses its registration.
launch_tree_ensure() { # <project> <tree>
    local project="$1" tree="$2"
    if [ -f "$tree/.git" ] \
        && [ "$(_launch_tree_common_dir "$tree")" = "$(_launch_tree_common_dir "$project")" ]; then
        return 0
    fi
    # Only an unregistered leftover reaches here, never a live tree.
    rm -rf "$tree"
    mkdir -p "$(dirname "$tree")" || return 1
    git -C "$project" worktree add --force --detach --no-checkout "$tree" HEAD >/dev/null 2>&1 \
        || { echo "ERROR: could not create the launch tree at $tree" >&2; return 1; }
}

# The working tree of <project> as a git tree object, beside the HEAD it
# extends. Prints "<head> <tree>". The live index is never written: a copy
# takes `git add -A`. A HEAD move during the add retries, so the pair is one
# consistent state.
_launch_tree_capture() { # <project>
    local project="$1" index scratch head after tree_id attempt=0
    index="$(git -C "$project" rev-parse --git-path index)" || return 1
    case "$index" in /*) ;; *) index="$project/$index" ;; esac
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/launch-tree.XXXXXX")" || return 1
    while [ "$attempt" -lt 5 ]; do
        attempt=$((attempt + 1))
        rm -f "$scratch/index"
        head="$(git -C "$project" rev-parse --verify HEAD)" || break
        [ ! -f "$index" ] || cp "$index" "$scratch/index" || break
        GIT_INDEX_FILE="$scratch/index" git -C "$project" add -A || break
        tree_id="$(GIT_INDEX_FILE="$scratch/index" git -C "$project" write-tree)" || break
        after="$(git -C "$project" rev-parse --verify HEAD)" || break
        if [ "$after" = "$head" ]; then
            rm -rf "$scratch"
            echo "$head $tree_id"
            return 0
        fi
    done
    rm -rf "$scratch"
    echo "ERROR: could not snapshot $project" >&2
    return 1
}

# Point <tree> at the working tree of <project>, and print the commit it
# extends. `read-tree -u --reset` rewrites only the files whose content
# changed, so the rest keep their mtimes and cargo stays incremental. Never
# `checkout --force`, which rewrites more and bumps mtimes.
launch_tree_snapshot() { # <project> <tree>
    local captured head tree_id
    captured="$(_launch_tree_capture "$1")" || return 1
    head="${captured%% *}"
    tree_id="${captured#* }"
    git -C "$2" update-ref --no-deref HEAD "$head" || return 1
    git -C "$2" read-tree -u --reset "$tree_id" || return 1
    echo "$head"
}

# Copy the gitignored VERSION and version stamp of each crate into a tree that
# has none, so the CalVer continues from the live checkout's. build.rs owns
# them from then on.
launch_tree_seed_versions() { # <project> <tree>
    local project="$1" tree="$2" version crate
    for version in "$project"/crates/*/VERSION; do
        [ -f "$version" ] || continue
        crate="${version#"$project"/}"
        crate="${crate%/VERSION}"
        [ -d "$tree/$crate" ] || continue
        [ ! -e "$tree/$crate/VERSION" ] || continue
        cp "$version" "$tree/$crate/VERSION" || return 1
        if [ -f "$project/$crate/.version_stamp" ]; then
            cp "$project/$crate/.version_stamp" "$tree/$crate/.version_stamp" || return 1
        fi
    done
}

# The command a build slot runs: snapshot and seed VERSION when <snapshot> is
# set, then exec <cmd> inside the tree. Cargo reads its config and toolchain from the working
# directory, so a --manifest-path into the tree is not enough.
launch_tree_exec_in() { # <project> <tree> <snapshot> <cmd...>
    local project="$1" tree="$2" snapshot="$3" head
    shift 3
    if [ -n "$snapshot" ]; then
        head="$(launch_tree_snapshot "$project" "$tree")" || return 1
        launch_tree_seed_versions "$project" "$tree" || return 1
        echo "Building $head from the launch tree $tree" >&2
    fi
    cd "$tree" || return 1
    exec "$@"
}

# A perl process that takes <lock> exclusively and holds it until a line
# arrives on <release>, or until its parent dies. It reports `waiting`,
# `locked` or `error` on <ready>. Both FIFOs open read-write, so no open
# blocks. Perl marks the lock descriptor close-on-exec and spawns nothing.
_launch_tree_hold() { # <lock> <ready> <release>
    # exec, so the parent perl watches is the build shell, not a subshell.
    exec perl -MFcntl=:flock -e '
        my ($lock, $ready, $release) = @ARGV;
        my $parent = getppid();
        open(my $r, "+<", $ready) or exit 2;
        $r->autoflush(1);
        my $fail = sub { print $r "error\n"; exit 2 };
        open(my $g, "+<", $release) or $fail->();
        open(my $fh, ">>", $lock) or $fail->();
        unless (flock($fh, LOCK_EX | LOCK_NB)) {
            print $r "waiting\n";
            until (flock($fh, LOCK_EX | LOCK_NB)) {
                exit 0 if getppid() != $parent;
                select(undef, undef, undef, 0.5);
            }
        }
        print $r "locked\n";
        my $rin = "";
        vec($rin, fileno($g), 1) = 1;
        while (getppid() == $parent) {
            my $rout = $rin;
            last if select($rout, undef, undef, 1) > 0;
        }
    ' "$1" "$2" "$3"
}

# Run <cmd> holding the launch tree lock of <project>, waiting for any holder.
# A helper process holds the lock, never this shell. Bash 3.2 lets a child
# inherit a descriptor even through `func 5>&-`, and a daemon such as the
# sccache server would then pin the lock for its whole life.
launch_tree_locked() { # <project> <cmd...>
    local lock scratch holder state="" rc=0
    _flock_need_perl || return 1
    lock="$(launch_tree_lock_path "$1")"
    shift
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/launch-tree-lock.XXXXXX")" || return 1
    mkfifo "$scratch/ready" "$scratch/release" || { rm -rf "$scratch"; return 1; }
    # Descriptor 5: 6 to 9 belong to stop.sh, web-dev.sh, ports.sh and the
    # build-watch.
    exec 5<>"$scratch/ready"
    _launch_tree_hold "$lock" "$scratch/ready" "$scratch/release" \
        </dev/null >/dev/null 2>&1 5>&- &
    holder=$!
    while read -r state <&5; do
        case "$state" in
            waiting) _flock_waiting "$lock" ;;
            *) break ;;
        esac
    done
    exec 5>&-
    if [ "$state" = locked ]; then
        "$@" || rc=$?
        echo go > "$scratch/release"
    else
        echo "ERROR: could not take the launch tree lock $lock" >&2
        rc=1
    fi
    wait "$holder" 2>/dev/null
    rm -rf "$scratch"
    return "$rc"
}
