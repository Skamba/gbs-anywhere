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

## How it works

After the knob press (on an E65/E80 GbS: after the Grind-by-Sync grind)
gbs-anywhere asks the cloud for new coffees every 3 s (the check interval,
`--lm-poll-s`; lower reports sooner but sends more requests).
Its first look notes what the log already holds; the first coffee logged
after that is the shot: its extraction time and its weight go to the grinder.
Without a scale reading it uses the recipe weight the grinder sent. The log
says which.

- **Only while the grinder waits for the shot,** after a knob press (on an
  E65/E80 GbS: after the Grind-by-Sync grind). Coffees made at other times
  are never reported, and a coffee already in the log at a knob press never
  counts for it. One the machine uploads only after the next knob press
  (the shot was typed in the app or timed out first) can still count for
  that next one.
- **Rather missed than wrong.** If the cloud can't be reached at the knob
  press, the first look that works counts as the first; a shot already
  logged by then is not reported, so type it in the app. Coffees are told
  apart by the cloud's own times, so the computer's clock doesn't matter
  for that.
- **A few seconds late.** The machine uploads the coffee when the shot ends;
  expect the grinder to react some seconds after the pump stops.
- **Flushes don't count.** The cloud logs flushes separately, and coffees it
  marks invalid are skipped, so the grinder keeps waiting for a real shot.
- **Same account as the app.** gbs-anywhere registers itself as one more
  installation of the La Marzocco Home app (derived from your credentials,
  nothing else is stored). The app keeps working alongside it.
- **Retries by itself.** After a connection problem it tries again within
  5 s to 5 min. A refused login, or a machine it can't pick (none on the
  account, the serial number not on it, or more than one and no serial
  number), is tried again within minutes at first and then after an hour,
  or at once when its settings are changed in the app.

## Official logo

The app shows a generic machine glyph, because vendor logos are trademarks
and are not shipped. To show La Marzocco's lion, put a monochrome
`la_marzocco.svg` or `.png` (transparent background) you are allowed to use
in a directory and pass `-v /that/dir:/icons:ro -e ICONS_DIR=/icons`; the app
tints it to match.
