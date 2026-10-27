# Build for users

**Always loaded:** this rule applies before planning or editing, regardless of file type.
This file is the single source for the population check.

Lucidos ships to every user, in every workspace.
The dev Mac is one test bench, never the target.
Its VPNs, Tailscale or Headscale, paths, ports, accounts and workspaces do not define the product.

A bug reported from this machine is a symptom.
Find the general cause and fix it for the whole reachable population.
Consider macOS, Linux, Windows, packaged and source installs, other networks, control servers and locales.
Assess relevant platforms without claiming support the product does not ship.

Do not match a value merely because it holds here when an authoritative source exists.
This includes address ranges, interface names, default prefixes, paths, hostnames and version strings.
Ask the tool's own CLI or API, the OS or the configuration instead of guessing from a heuristic.
If no authority exists, state that in the plan and name who the heuristic fails for.

Every plan and final report must state the reachable population: who the fix works for, who it excludes, and why.
"Works on my setup" is not a pass.

For example, the pairing QR fix selected Tailscale's interface by its default IPv6 prefix.
That guess fit one Mac with stock Tailscale.
It did not establish coverage for Headscale, IPv4-only tailnets, Linux, Windows or other VPN layouts.
Ask Tailscale's own CLI or API for its address instead of treating the default prefix as proof of interface ownership.
