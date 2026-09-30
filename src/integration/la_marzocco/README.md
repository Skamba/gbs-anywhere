# La Marzocco cloud

Connected La Marzocco machines log every coffee to the La Marzocco cloud with
the extraction time and, with a Connected Scale, the weight in the cup: the
numbers the machine shows after a shot. gbs-anywhere can log in with your
La Marzocco Home account and report shots from that log, so nothing needs to
be typed. It works on any connected La Marzocco, including the Linea Mini R,
whose live brewing state never reaches the cloud, and it reports the weight
the machine measured rather than a target.

## Set it up

In the app: the green **+** at the top right → **La Marzocco cloud**, then enter your La Marzocco
Home e-mail address and password. Add the machine's serial number (e.g.
`MI000000`) if the account has more than one machine.

Or on the command line / in Docker:

```sh
docker run -d --name gbs-anywhere --restart unless-stopped -p 80:80 \
  -e LM_USERNAME=you@example.com \
  -e LM_PASSWORD='your La Marzocco Home password' \
  ghcr.io/skamba/gbs-anywhere
```

| setting | flag | env | default |
|---|---|---|---|
| E-mail | `--lm-username` | `LM_USERNAME` | off |
| Password | `--lm-password` | `LM_PASSWORD` | |
| Machine serial number | `--lm-serial` | `LM_SERIAL` | the account's only machine |
| Seconds between checks (1 to 60) | `--lm-poll-s` | `LM_POLL_S` | 3 |

## How it works

After the knob press gbs-anywhere asks the cloud for new coffees every 3 s.
The first coffee logged after the knob press is the shot: its extraction time
and its weight go to the grinder. Without a scale reading it uses the recipe
weight the grinder sent. The log says which.

- **Only after a knob press.** Coffees made while the grinder is not waiting
  are never reported.
- **A few seconds late.** The machine uploads the coffee when the shot ends;
  expect the grinder to react some seconds after the pump stops.
- **Flushes don't count.** The cloud logs flushes separately, and coffees it
  marks invalid are skipped, so the grinder keeps waiting for a real shot.
- **Same account as the app.** gbs-anywhere registers itself as one more
  installation of the La Marzocco Home app (derived from your credentials,
  nothing else is stored). The app keeps working alongside it.
- **The phone still works.** You can still enter or correct a shot on your
  phone; whichever comes first wins.
- The cloud login retries by itself with backoff.

## Official logo

The app shows a generic machine glyph, because vendor logos are trademarks
and are not shipped. To show La Marzocco's lion, put a monochrome
`la_marzocco.svg` or `.png` (transparent background) you are allowed to use
in a directory and pass `-v /that/dir:/icons:ro -e ICONS_DIR=/icons`; the app
tints it to match.
