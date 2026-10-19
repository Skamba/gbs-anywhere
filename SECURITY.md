# Security

gbs-anywhere is meant for a home network. The app and its API have no login:
anyone on your network who can open the app can report shots, add or remove
integrations and change the settings, and any device on the network can talk
to it as the grinder does. Don't expose it to the internet.

Web pages are kept out: the app answers only to its IP address and
home-network host names (more with `--allow-host`), refuses changes requested
from other sites, and the grinder's routes refuse changes from browsers. The
settings file (with the integrations' passwords), the E65/E80 pairing file and
the shot history are readable by their owner only (on Linux and macOS, where
the file system keeps file modes; some shares don't, and the log then says
so).

Accepted on purpose:
- The grinder's routes answer the grinder by the computer's IP address, so
  they don't check the host name. A page that re-points its own name to
  that address can read the machine state they report (nothing private),
  but not change it.
- Browsers send `Sec-Fetch-Site` only over HTTPS, and a plain read carries
  no `Origin`, so over the home network's plain HTTP a page's reads look
  like the grinder's. Any page can make them (an image pointing at the
  grinder's routes is enough), though only one that re-points its own name
  gets to see the reply. Such reads count as the grinder's polls, so Status
  may show the phone as the grinder that polled last. Only that status is
  affected; nothing changes. Every change a page asks for carries `Origin`
  and is refused.
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
