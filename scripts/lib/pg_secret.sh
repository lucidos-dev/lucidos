#!/usr/bin/env bash
# pg_secret.sh: the password of the shared dev Postgres cluster.
#
# Docker Desktop cannot hand a Unix socket from its VM to the host. So the dev
# cluster listens on loopback TCP, which every local user can reach, and a
# minted secret keeps them out. Design:
# docs/plans/2026-10-12-other-local-users-cannot-reach-postgres-or-devtools.md.
#
# The gateway mints the same file through `lucidos_local_token::ensure_named`,
# and this follows its protocol step for step, so the two never publish
# different secrets:
#
#   1. Hold an exclusive flock on `.<file>.<MINT_LOCK_SUFFIX>` beside the file.
#   2. A non-empty file is the answer. An empty one was never published, and
#      one that cannot be read is an error, never minted over.
#   3. Otherwise write TOKEN_BYTES of entropy as hex to a 0600 temp file, and
#      rename it over the file.
#
# Perl holds the flock because macOS ships no `flock` command, as in
# scripts/lib/e2e_lock.sh. Never put the result in argv: `ps` shows argv to
# every local user. Hand it to a child through its environment instead.

# shellcheck source=scripts/lib/workspace_constants.sh
source "$(dirname "${BASH_SOURCE[0]}")/workspace_constants.sh"
# shellcheck source=scripts/lib/storage_root.sh
source "$(dirname "${BASH_SOURCE[0]}")/storage_root.sh"

# shared_pg_password: print the secret, minting it on first use. The Lucidos
# home it lives in is a storage root, never open even briefly (ADR 0456).
shared_pg_password() {
    local dir
    dir="$(lucidos_home_dir)"
    ensure_storage_root "$dir" || return 1
    perl -MFcntl=:DEFAULT,:flock -e '
        my ($dir, $name, $suffix, $bytes) = @ARGV;
        my ($path, $lock) = ("$dir/$name", "$dir/.$name.$suffix");
        sysopen(my $mutex, $lock, O_WRONLY | O_CREAT) or die "cannot open $lock: $!\n";
        flock($mutex, LOCK_EX) or die "cannot lock $lock: $!\n";
        if (-e $path) {
            open(my $published, "<", $path) or die "cannot read $path: $!\n";
            my $secret = do { local $/; <$published> } // "";
            $secret =~ s/\s+//g;
            if (length $secret) {
                chmod(0600, $path) or die "cannot chmod $path: $!\n";
                print "$secret\n";
                exit 0;
            }
        }
        open(my $random, "<", "/dev/urandom") or die "cannot read /dev/urandom: $!\n";
        read($random, my $raw, $bytes) == $bytes or die "short read from /dev/urandom\n";
        my $secret = unpack("H*", $raw);
        my $staged = "$dir/.$name.$$.tmp";
        sysopen(my $out, $staged, O_WRONLY | O_CREAT | O_EXCL, 0600) or die "cannot create $staged: $!\n";
        print $out $secret or die "cannot write $staged: $!\n";
        close($out) or die "cannot write $staged: $!\n";
        rename($staged, $path) or die "cannot publish $path: $!\n";
        print "$secret\n";
    ' "$dir" "$SHARED_PG_PASSWORD_FILE" "$MINT_LOCK_SUFFIX" "$TOKEN_BYTES"
}
