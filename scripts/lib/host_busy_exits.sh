# shellcheck shell=bash disable=SC2034 # read by the libraries and scripts that source this file
# host_busy_exits.sh: the exit codes that mean "this host was too busy to run".
# None is a test verdict. e2e_github_handoff reads them to send a filtered run
# to GitHub, and every gate that returns one sources this file.

# A memory stop inside a run. 71 is sysexits' EX_OSERR: an OS resource
# condition. Playwright exits 0, 1 or 130, so 71 never collides with it.
HOST_MEMORY_STOP_EXIT=71
# The build memory gate refused: the build never started, so nothing was tested.
HOST_MEMORY_BUILD_REFUSED_EXIT=72
# The host-load guard still read saturated past its wait cap, and a run found
# every e2e slot held. EX_TEMPFAIL: the run did not happen. The nightly
# orchestrator keys on 75, and tests may override the host-load half.
: "${HOST_LOAD_SATURATED_EXIT:=75}"
E2E_LOCK_BUSY_EXIT=75
