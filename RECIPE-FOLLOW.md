# Experimental recipe following on a computer or NAS

This fork adds optional recipe-target selection to the existing Rust
gbs-anywhere app. It runs on a computer with a browser or in a Docker
container; no ESP board is required. The web interface is in English and
German. This is separate from the Chinese native UI in the ESP firmware.

The implementation is experimental and has not yet completed a real-cup
test. Earlier ESP tests are not validation of this desktop implementation.
Mac/PC/Docker/NAS are deployment options, not a claim that each has passed
hardware testing; Windows has not been tested with a real grinder here.

Source:
[`zpmdd/gbs-anywhere`, `codex/desktop-recipe-follow`](https://github.com/zpmdd/gbs-anywhere/tree/codex/desktop-recipe-follow).
There is no prebuilt image or binary release for this feature. Upstream
`latest`, `nightly`, Debian packages and release downloads do not include it.

## What the automation does

1. A fresh accepted E65/E80 GbS grind identifies its recipe by the exact grind
   UUID. The app reads the grinder's recipe export using its Owner password
   and resolves the matching recipe's beverage target.
2. While you prepare the puck, the app selects an **existing** La Marzocco
   Brew-by-Weight Dose 1 or Dose 2 with that target. If neither matches, it
   saves the recipe target to **Dose 1** and selects it, preserving Dose 2.
   **Enabling recipe following permits this persistent change to Dose 1.**
   The new weight remains after the shot and when following is turned off.
   Ambiguous matches still require a manual check.
3. Wait for **target … g confirmed** on Start, then check the selected target
   on the machine before operating its brew lever. The app never starts a
   shot. It has no reliable live brew-start signal for every machine.
4. The existing cloud integration waits for the completed coffee and sends
   its recorded extraction time and measured weight to the grinder. The
   grinder decides whether and how to adjust its grind setting. With recipe
   following enabled, a coffee without a measured scale weight is not
   reported automatically; the target weight is never substituted for it.

Selection is allowed only shortly after a fresh grind (a 15-second permit,
with at most 8 seconds for the control operation). Jobs already present when
the app starts do not trigger selection. If a change cannot be confirmed,
the app does not replay it automatically: check the machine. A missing
reliable coffee-log baseline skips target changes and automatic feedback
for that grind. Multiple La Marzocco integrations disable automatic target
selection to avoid choosing the wrong machine.
The cloud check must also show that the machine is powered on and the scale
is connected. A known active brew prevents target selection.
The cloud may upload a previous coffee late enough to cross the next
baseline. Wait for each cup's feedback before starting another; the
baseline cannot prove away every delayed upload.

The original manual entry and cloud-feedback behavior remain available
with recipe following off. In that mode only, the existing feedback path
can fall back to the grinder's recipe weight without a measured scale
reading. That fallback is not a measurement and must not be counted as a
successful measured-weight test.

## Before enabling it

- A paired **E65/E80 GbS** grinder with Local WiFi and its Owner password.
  The E64 WS manual/cloud-feedback flow remains available, but it is not
  covered by this automatic recipe selection feature.
  Recipe exports use HTTPS on port 443 at the same hostname as the
  configured grinder address. The Local WiFi port can be configured, but a
  separate export host or port is not supported in this version.
- A connected La Marzocco, its Home account, and a working connected
  Brew-by-Weight scale. The machine must expose Dose 1 and Dose 2 targets
  through the cloud. They need not already match the grinder's recipes.
  Machine and scale combinations
  still need real-device confirmation.
- One running gbs-anywhere instance for this grinder. Stop other bridges,
  ESP integrations and competing grinder clients before testing. Do not
  delete or reset an existing pairing just to switch clients.
- The computer/container can reach the grinder on the home network and the
  La Marzocco cloud. Keep the computer awake while making coffee.
- Use the upstream **Set up grinder** flow for initial pairing. A saved
  pairing and settings directory should be reused across restarts.

## Build and run from source

Install Git and current stable Rust (at least the version in `Cargo.toml`).
Native PC use requires the Rust toolchain and its platform build tools;
Docker Desktop is the alternative on Mac or Windows. These commands build
this branch, not an upstream release:

```sh
git clone --branch codex/desktop-recipe-follow https://github.com/zpmdd/gbs-anywhere.git gbs-anywhere-recipe-follow
cd gbs-anywhere-recipe-follow
mkdir ../gbs-anywhere-data
cargo run --release --locked -- -p 8089 --control off --no-stdin --config ../gbs-anywhere-data/settings.json --update-check off
```

Create the data directory only on the first run. Keep it outside the source
checkout so cleaning build output does not remove settings or pairing.
Open `http://localhost:8089/` on the computer, or
`http://<computer's LAN IP>:8089/` on a phone on the same network. Allow this
port on the private network if the firewall asks. E65/E80 connects through
outgoing requests to the grinder, so the app can use port 8089.

## Docker or NAS Compose

Clone the branch as above, then build the image locally:

```sh
docker build -t gbs-anywhere-recipe-follow:local .
docker run -d --name gbs-anywhere-recipe-follow --restart unless-stopped -p 8089:80 -v gbs-recipe-follow-data:/data gbs-anywhere-recipe-follow:local --update-check off
```

Alternatively, the included Compose file builds the same Dockerfile and
keeps settings, history and pairing in the same `gbs-recipe-follow-data`
named volume as the `docker run` example:

```sh
docker compose -f compose.recipe-follow.yml up -d --build
```

For a NAS, copy/clone the complete source checkout and run this command in
that directory, or import the Compose file into a manager that supports
local `build` contexts. A manager that only pulls images needs you to build
and transfer the image yourself first. Open `http://<NAS LAN IP>:8089/`.
Do not run the native, Docker and Compose examples simultaneously for the
same grinder. Keep the volume when recreating the container; avoid
`docker compose down -v`, which deletes it.

The app has no login and is intended for a trusted home network. Do not
forward its port to the internet. On a host with several networks, bind the
published port to its home-network IP. Do not place passwords in a public
Compose file, GitHub comment or diagnostic archive.

## Configure it in the browser

1. Use **Set up grinder** on Start and complete the upstream E65/E80 pairing
   flow. With an existing configuration, first confirm that the grinder is
   connected.
2. Under **Status → Machine integrations → + → La Marzocco cloud**, enter
   the account details and machine serial if the account has several
   machines. Set up only one La Marzocco integration for recipe following.
3. Keep recipe following off until the normal cloud status is healthy.
   Check the connected scale works and note the machine's Dose 1 and Dose 2
   weights. An unmatched recipe will replace Dose 1's saved weight.
4. Edit the integration, enter the **Grinder Owner password**, enable
   **Follow the grinder recipe (experimental)** and save. The Owner password
   belongs to the grinder, not the La Marzocco account. It is stored with the
   integration settings and never returned by the settings API. Enabling
   the option also allows persistent updates to Dose 1 when neither preset
   matches the recipe target; Dose 2 is kept.
5. Return to **Start**. The **Machine status before brewing** card shows the
   integration detail without opening Settings. An error or an unconfirmed
   target means you must select and check the target yourself.

The CLI equivalents are `--lm-recipe-follow` (`LM_RECIPE_FOLLOW=true`) and
`--lm-grinder-owner-password` (`LM_GRINDER_OWNER_PASSWORD`), together with the
existing `--lm-username` and `--lm-password`. Prefer the form so passwords do
not appear in command history or container environment inspection.

## First test and feedback

Before making a cup, note the grinder, machine and scale models and their
firmware versions. Use two grinder recipes whose beverage targets match the
machine's two existing presets, for example 20 g and 30 g if that suits your
setup. Confirm the first target, pull one shot, and wait for the cloud result
and grinder feedback before changing recipes. Then repeat in the other
direction. Next try a target that matches neither preset: confirm that
Dose 1 changes to that target and Dose 2 keeps its original weight. Check
the actual cup weight against the scale; do not use the
target value as proof of a measured result.

Report which recipe direction was tried, whether the target was confirmed
on Start and on the machine, the measured time/weight returned, and the
grinder's response. If selection is unconfirmed, check the machine manually
and report that outcome; do not keep toggling the feature to repeat a write.
Redact account names, serials, IP addresses, passwords, tokens and pairing
data. Do not upload the settings file, exported recipes or raw device logs.

To stop following recipes, uncheck the option and save. Cloud feedback
continues as before; the saved Owner password remains for a later opt-in.
Existing changes to Dose 1 are not undone; restore its weight on the
machine if needed. Deleting the integration removes its saved passwords.
