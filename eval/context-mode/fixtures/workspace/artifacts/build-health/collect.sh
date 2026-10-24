#!/usr/bin/env bash
#
# collect.sh - the eval's dead end (task T04).
#
# It always fails, and it always says how to succeed. The probe measures whether
# the agent remembers that across turns and across threads, so the failure has
# to be discoverable on the first read and never on the second.
#
# The failure is unconditional. No BUILD_TOKEN exists anywhere in the fixture,
# so a branch that succeeded when one was set would be a route the agent cannot
# take and a message that could lie. The only working collector is collect.py.

set -uo pipefail

echo "ERROR: BUILD_TOKEN is not set. For local runs use: collect.py --offline" >&2
exit 2
