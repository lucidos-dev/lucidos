# shellcheck shell=bash
# The engine input manifest: every file cargo read to build the e2e binaries,
# with its git blob id, so a later run can tell whether a built engine still
# matches the tree under test (ADR 0536).
#
# A manifest is plain text, one entry per line, tab-separated:
#
#   lucidos-engine-input-manifest	1
#   key	<name>	<value>        profile, features, platform, rustflags, toolchain override
#   file	<blob id>	<path>     a file a binary's dep-info names
#   dir	<hash>	<path>         a rerun-if-changed directory, hashed over its files
#
# Paths are relative to the repository root. Only files git sees count: tracked
# ones and untracked ones that are not ignored. A file the build itself writes
# into the tree (the engine's gitignored VERSION) is an output, not an input.
# Paths outside the tree (registry crates, the git directory) are pinned by
# Cargo.lock and the toolchain file, which every manifest lists.
#
# One mechanism serves GitHub, where the build job uploads the manifest beside
# the binaries, and this host, where the local engine cache keeps them. Tested
# by scripts/lib/engine_manifest_test.sh.

# The binaries a manifest covers, as cargo names them in its profile directory.
ENGINE_MANIFEST_BINARIES=(lucidos-engine lucidos-gateway lucidos lucidos-codegen)

# How many builds the local engine cache keeps, newest first.
ENGINE_CACHE_KEEP=4

# `<uname -s>-<uname -m>`, the platform a binary runs on.
engine_manifest_platform() {
    echo "$(uname -s)-$(uname -m)"
}

# engine_manifest_keys <profile> <features> [<platform>]: the key lines. GitHub's
# plan job passes the platform of the runner it checks for.
engine_manifest_keys() {
    local profile="$1" features="$2" platform="${3:-$(engine_manifest_platform)}"
    printf 'key\tprofile\t%s\n' "$profile"
    printf 'key\tfeatures\t%s\n' "${features:--}"
    printf 'key\tplatform\t%s\n' "$platform"
    printf 'key\trustflags\t%s\n' "${RUSTFLAGS:--}"
    printf 'key\ttoolchain-override\t%s\n' "${RUSTUP_TOOLCHAIN:--}"
}

_engine_manifest_py() {
    python3 - "$@" <<'PY'
import hashlib, os, subprocess, sys

HEADER = "lucidos-engine-input-manifest\t1"
# Inputs every build reads, whatever dep-info says: the dependency and
# toolchain pins, and the workspace profile settings.
ALWAYS = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml"]

def blob(path):
    data = open(path, "rb").read()
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()

def visible(repo):
    out = subprocess.run(
        ["git", "-C", repo, "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        check=True, capture_output=True).stdout
    return {p for p in out.decode().split("\0") if p and os.path.isfile(os.path.join(repo, p))}

def dir_hash(repo, files, rel):
    h = hashlib.sha1()
    for p in sorted(f for f in files if f.startswith(rel + "/")):
        h.update(f"{p}\t{blob(os.path.join(repo, p))}\n".encode())
    return h.hexdigest()

def depinfo_paths(path):
    text = open(path).read().replace("\\\n", " ")
    for line in text.splitlines():
        head, sep, deps = line.partition(": ")
        if not sep:
            continue
        token, escaped = "", False
        for ch in deps + " ":
            if escaped:
                token += ch
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == " ":
                if token:
                    yield token
                token = ""
            else:
                token += ch

def package_root(repo, rel):
    d = os.path.dirname(rel)
    while d:
        if os.path.isfile(os.path.join(repo, d, "Cargo.toml")):
            return d
        d = os.path.dirname(d)
    return None

def write(repo, profile_dir, keys, out, binaries):
    repo = os.path.realpath(repo)
    files = visible(repo)
    found_files, found_dirs = set(), set()
    built_at = None
    for name in binaries:
        depinfo = os.path.join(profile_dir, name + ".d")
        mtime = os.path.getmtime(depinfo)
        built_at = mtime if built_at is None else min(built_at, mtime)
        for p in depinfo_paths(depinfo):
            real = os.path.realpath(p)
            if not real.startswith(repo + "/"):
                continue
            rel = real[len(repo) + 1:]
            if rel in files:
                found_files.add(rel)
            elif os.path.isdir(real) and not rel.startswith((".git/", "target/")):
                found_dirs.add(rel)
    for rel in list(found_files):
        root = package_root(repo, rel)
        if root:
            for extra in ("Cargo.toml", "build.rs"):
                if os.path.join(root, extra) in files:
                    found_files.add(os.path.join(root, extra))
    found_files.update(p for p in ALWAYS if p in files)
    # Cargo's own freshness rule: an input newer than the dep-info changed
    # after the build read it, so the binaries describe an older tree.
    inputs = set(found_files)
    for d in found_dirs:
        inputs.update(f for f in files if f.startswith(d + "/"))
    for p in sorted(inputs):
        if os.path.getmtime(os.path.join(repo, p)) > built_at:
            sys.exit(f"{p} changed after the build read it")
    lines = [HEADER] + keys
    lines += [f"file\t{blob(os.path.join(repo, p))}\t{p}" for p in sorted(found_files)]
    lines += [f"dir\t{dir_hash(repo, files, d)}\t{d}" for d in sorted(found_dirs)]
    with open(out, "w") as f:
        f.write("\n".join(lines) + "\n")

def check(repo, manifest, keys):
    repo = os.path.realpath(repo)
    lines = open(manifest).read().splitlines()
    if not lines or lines[0] != HEADER:
        return "not an engine input manifest"
    have_keys = [l for l in lines[1:] if l.startswith("key\t")]
    if have_keys != keys:
        want = set(keys) ^ set(have_keys)
        return "built for another configuration: " + ", ".join(sorted(want))
    files = visible(repo)
    for line in lines[1:]:
        kind, _, rest = line.partition("\t")
        if kind == "key":
            continue
        digest, _, rel = rest.partition("\t")
        if kind == "file":
            if rel not in files:
                return f"{rel} is gone"
            if blob(os.path.join(repo, rel)) != digest:
                return f"{rel} changed"
        elif kind == "dir":
            if dir_hash(repo, files, rel) != digest:
                return f"a file in {rel}/ changed, appeared or went"
        else:
            return f"unknown entry: {line}"
    return None

# Restored target/ state is fresh to cargo only where every input is older
# than its outputs. Unchanged inputs get a fixed old time; everything else,
# including any file the manifest does not list, gets now.
OLD = 946684800  # 2000-01-01T00:00:00Z

def stamp(repo, manifest):
    repo = os.path.realpath(repo)
    files = visible(repo)
    unchanged = set()
    for line in open(manifest).read().splitlines()[1:]:
        kind, _, rest = line.partition("\t")
        digest, _, rel = rest.partition("\t")
        if kind == "file" and rel in files and blob(os.path.join(repo, rel)) == digest:
            unchanged.add(rel)
        elif kind == "dir" and dir_hash(repo, files, rel) == digest:
            unchanged.update(f for f in files if f.startswith(rel + "/"))
    for rel in files:
        when = OLD if rel in unchanged else None
        os.utime(os.path.join(repo, rel), None if when is None else (when, when),
                 follow_symlinks=False)
    print(f"{len(unchanged)} unchanged, {len(files) - len(unchanged)} stamped now")

# The key lines arrive in the environment: the script itself is on stdin.
KEYS = os.environ.get("ENGINE_MANIFEST_KEYS", "").splitlines()
cmd = sys.argv[1]
if cmd == "write":
    repo, profile_dir, out = sys.argv[2:5]
    write(repo, profile_dir, KEYS, out, sys.argv[5:])
elif cmd == "check":
    repo, manifest = sys.argv[2:4]
    reason = check(repo, manifest, KEYS)
    if reason:
        print(reason, file=sys.stderr)
        sys.exit(1)
elif cmd == "stamp":
    stamp(sys.argv[2], sys.argv[3])
elif cmd == "blob":
    print(blob(sys.argv[2]))
PY
}

# engine_manifest_stamp_mtimes <repo> <manifest>: set the tree's mtimes so
# cargo reuses restored target/ state for exactly the inputs that match
# <manifest>, the manifest of the build that left that state.
engine_manifest_stamp_mtimes() {
    _engine_manifest_py stamp "$1" "$2"
}

# engine_manifest_write <repo> <profile-dir> <out> <profile> <features>: write
# the manifest of the binaries cargo just built into <profile-dir>.
engine_manifest_write() {
    local repo="$1" profile_dir="$2" out="$3" profile="$4" features="$5"
    ENGINE_MANIFEST_KEYS="$(engine_manifest_keys "$profile" "$features")" \
        _engine_manifest_py write "$repo" "$profile_dir" "$out" "${ENGINE_MANIFEST_BINARIES[@]}"
}

# engine_manifest_check <repo> <manifest> <profile> <features> [<platform>]:
# 0 when the binaries the manifest describes were built from exactly this tree
# and configuration. Otherwise 1, with the first difference on stderr.
engine_manifest_check() {
    local repo="$1" manifest="$2" profile="$3" features="$4" platform="${5:-}"
    ENGINE_MANIFEST_KEYS="$(engine_manifest_keys "$profile" "$features" ${platform:+"$platform"})" \
        _engine_manifest_py check "$repo" "$manifest"
}

# engine_manifest_prekey <repo> <profile> <features> [<platform>]: a short hash
# of what every matching manifest shares, the configuration and the two pins.
# GitHub artifact names carry it, so a lookup reads only candidates that can
# match.
engine_manifest_prekey() {
    local repo="$1" profile="$2" features="$3" platform="${4:-}" pin
    {
        engine_manifest_keys "$profile" "$features" ${platform:+"$platform"}
        for pin in Cargo.lock rust-toolchain.toml; do
            _engine_manifest_py blob "$repo/$pin"
        done
    } | git hash-object --stdin | cut -c1-12
}

# ── The local engine cache ──────────────────────────────────────────────
# Builds filed under ~/.lucidos/e2e-engines/<manifest id>/, shared by every
# checkout on this host, so a worktree whose Rust matches another's build
# reuses it instead of compiling.

engine_cache_root() {
    echo "${LUCIDOS_E2E_ENGINE_CACHE:-$HOME/.lucidos/e2e-engines}"
}

# engine_cache_store <repo> <profile-dir> <profile> <features>: file the
# binaries cargo built into <profile-dir> under their manifest. The copies come
# first and the manifest is written from the copied dep-info, so a build that
# lands in <profile-dir> meanwhile cannot pair one build's binaries with
# another's manifest.
engine_cache_store() {
    local repo="$1" profile_dir="$2" profile="$3" features="$4"
    local root stage name id
    root="$(engine_cache_root)"
    mkdir -p "$root" || return 1
    stage="$(mktemp -d "$root/.staging.XXXXXX")" || return 1
    for name in "${ENGINE_MANIFEST_BINARIES[@]}"; do
        if ! { cp -c "$profile_dir/$name" "$stage/" 2>/dev/null || cp "$profile_dir/$name" "$stage/"; } \
            || ! cp -p "$profile_dir/$name.d" "$stage/"; then
            rm -rf "$stage"
            return 1
        fi
    done
    for name in "${ENGINE_MANIFEST_BINARIES[@]}"; do
        if ! cmp -s "$profile_dir/$name" "$stage/$name"; then
            echo "Engine cache: $name changed while it was filed, so this build is not cached."
            rm -rf "$stage"
            return 1
        fi
    done
    engine_manifest_write "$repo" "$stage" "$stage/manifest" "$profile" "$features" || {
        rm -rf "$stage"
        return 1
    }
    rm -f "$stage"/*.d
    id="$(git hash-object "$stage/manifest" | cut -c1-16)"
    if [ -d "$root/$id" ]; then
        rm -rf "$stage"
        touch "$root/$id"
    else
        mv "$stage" "$root/$id" || { rm -rf "$stage"; return 1; }
    fi
    engine_cache_prune "$root"
    echo "Engine cache: filed this build as $id."
}

# engine_cache_prune <root>: keep the newest ENGINE_CACHE_KEEP builds.
engine_cache_prune() {
    local root="$1" dir n=0
    # shellcheck disable=SC2045 # cache entries are hex ids, never spaces
    for dir in $(ls -1t "$root"); do
        [ -f "$root/$dir/manifest" ] || continue
        n=$((n + 1))
        [ "$n" -le "$ENGINE_CACHE_KEEP" ] || rm -rf "${root:?}/$dir"
    done
}

# engine_cache_find <repo> <profile> <features>: print the directory of the
# newest cached build whose manifest matches the tree, or nothing.
engine_cache_find() {
    local repo="$1" profile="$2" features="$3" root dir
    root="$(engine_cache_root)"
    [ -d "$root" ] || return 0
    # shellcheck disable=SC2045 # cache entries are hex ids, never spaces
    for dir in $(ls -1t "$root"); do
        [ -f "$root/$dir/manifest" ] || continue
        if engine_manifest_check "$repo" "$root/$dir/manifest" "$profile" "$features" 2>/dev/null; then
            touch "$root/$dir"
            echo "$root/$dir"
            return 0
        fi
    done
}
