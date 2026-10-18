# Security

gbs-anywhere is meant for a home network. The app and its API have no login:
anyone on your network who can open the app can report shots, add or remove
integrations and change the settings, and any device on the network can talk
to it as the grinder does. Don't expose it to the internet.

Web pages are kept out: the app answers only to its IP address and
home-network host names (more with `--allow-host`), refuses changes requested
from other sites, and the grinder's routes refuse changes from browsers. The
settings file (with the integrations' passwords), the E65/E80 pairing file and
the shot history are readable by their owner only (on Linux and macOS).

Accepted on purpose:
- The grinder's routes answer the grinder by the computer's IP address, so
  they don't check the host name. A page that re-points its own name to
  that address can read the machine state they report (nothing private),
  but not change it.
- In a browser that sends neither `Origin` nor `Sec-Fetch-Site` (Safari
  before 16.4), such a page's reads count as the grinder's polls, so Status
  may show it as the grinder that polled last. Only that status is
  affected, nothing changes, and current browsers send these headers.
- The app may be shown in a frame, so that a dashboard such as Home
  Assistant can embed it. Another page could frame it too; removing an
  integration still asks you to type DELETE.
- What leaves your network: the update check asks GitHub once a day for the
  latest release (turn it off under Status → Settings, or with
  `--update-check off`), and an integration talks to its vendor's service,
  such as the La Marzocco cloud. The link to an E65/E80 grinder never goes
  through a proxy.

## Reporting a problem

Please report security problems privately through
[GitHub's private reporting](https://github.com/Skamba/gbs-anywhere/security/advisories/new),
not in a public issue. We'll reply there.

Only the newest release gets fixes.
