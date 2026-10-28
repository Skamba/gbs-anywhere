# La Marzocco cloud

Connected La Marzocco machines log every coffee to the La Marzocco cloud with
the extraction time and, with a Connected Scale, the weight in the cup: the
numbers the machine shows after a shot. gbs-anywhere can log in with your
La Marzocco Home account and report shots from that log, so nothing needs to
be typed. It works on any connected La Marzocco, including the Linea Mini R,
whose live brewing state never reaches the cloud, and it reports the weight
the machine measured rather than a target.

## Set it up

In the app: Status → Machine integrations → **+** → **La Marzocco
cloud**, then enter your La Marzocco Home e-mail address and password. Add the
machine's serial number (e.g. `MI000000`) if the account has more than one
machine.

On the experimental `codex/desktop-recipe-follow` branch, the same form also
offers **Follow the grinder recipe (experimental)**, off by default, and
**Grinder Owner password**. This selects an already configured matching
Brew-by-Weight Dose 1 or Dose 2 after a fresh E65/E80 GbS grind. If neither
matches, it saves the recipe target to Dose 1 and selects it, preserving
Dose 2. Enabling it permits this persistent change to Dose 1; it never
starts brewing. Read the [recipe-follow setup and
testing guide](../../../RECIPE-FOLLOW.md) before enabling it; this desktop
implementation has not yet been validated with a real cup. Its source-build
instructions replace the upstream image command below for this feature.

Setting it up in the app is the better way: the password is then kept in
the settings file on the `/data` volume, readable by its owner only, rather
than in the container's environment, which `docker inspect` shows.
Or on the command line / in Docker, with the `docker run` from
[Run it](../../../README.md#run-it):

```sh
docker run -d --name gbs-anywhere --restart unless-stopped \
  -p 80:80 -v gbs-anywhere:/data \
  -e LM_USERNAME=you@example.com \
  -e LM_PASSWORD='your La Marzocco Home password' \
  ghcr.io/skamba/gbs-anywhere
```

An integration set up this way can be moved into the app later, with all
its settings.

| setting | flag | env | default |
|---|---|---|---|
| E-mail | `--lm-username` | `LM_USERNAME` | off |
| Password | `--lm-password` | `LM_PASSWORD` | |
| Machine serial number | `--lm-serial` | `LM_SERIAL` | the account's only machine |
| Check interval (seconds, 1 to 60) | `--lm-poll-s` | `LM_POLL_S` | 3 |
| Follow the grinder recipe (experimental) | `--lm-recipe-follow` | `LM_RECIPE_FOLLOW` | false |
| Grinder Owner password | `--lm-grinder-owner-password` | `LM_GRINDER_OWNER_PASSWORD` | unset |

Recipe following needs the Owner password only when enabled. The app never
returns either password through its API; while editing, leaving a saved
password blank keeps it. Turning the checkbox off retains the saved Owner
password but stops recipe following. Delete the integration to remove its
stored passwords. `LM_RECIPE_FOLLOW=true` enables the CLI setting; keep
passwords out of shell history and shared Compose files.

## How it works

After the knob press (on an E65/E80 GbS: after the Grind-by-Sync grind)
gbs-anywhere asks the cloud for new coffees every 3 s (the check interval,
`--lm-poll-s`; lower reports sooner but sends more requests).
The first coffee logged after the knob press is the shot: its extraction time
and its weight go to the grinder. With recipe following off, a missing scale
reading falls back to the recipe weight the grinder sent; the log says which.
With experimental recipe following on, a coffee without measured scale
weight is skipped instead, and Start explains why.

- **Only while the grinder waits for the shot,** after a knob press (on an
  E65/E80 GbS: after the Grind-by-Sync grind). Coffees made at other times
  are never reported.
- **A few seconds late.** The machine uploads the coffee when the shot ends;
  expect the grinder to react some seconds after the pump stops.
- **Flushes don't count.** The cloud logs flushes separately, and coffees it
  marks invalid are skipped, so the grinder keeps waiting for a real shot.
- **Same account as the app.** gbs-anywhere registers itself as one more
  installation of the La Marzocco Home app (derived from your credentials,
  nothing else is stored). The app keeps working alongside it.
- The cloud login retries by itself with backoff.

## Official logo

The app shows a generic machine glyph, because vendor logos are trademarks
and are not shipped. To show La Marzocco's lion, put a monochrome
`la_marzocco.svg` or `.png` (transparent background) you are allowed to use
in a directory and pass `-v /that/dir:/icons:ro -e ICONS_DIR=/icons`; the app
tints it to match.
