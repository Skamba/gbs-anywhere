---
description: Check a change to the gbs-anywhere phone app (web/) in the browser at phone width, light and dark, against the review checklist in AGENTS.md. Use after editing web/index.html, web/app.css, web/app.js or web/strings/.
---

Check the phone app after a change in `web/`. The files are compiled into
the binary, so the server has to be rebuilt and restarted first; a browser
reload alone shows the old page.

1. Start or restart the preview from `.claude/launch.json`
   (`gbs-anywhere`, port 8089, settings in `target/dev.json`). Read the
   server log for errors.
2. Set the viewport to 390 × 844, light colour scheme. Go through each
   screen the change touches. Read the console for errors.
3. Do the same in the dark colour scheme.
4. For each item under "Done means" in AGENTS.md (after a gap, requests and
   live events, live regions, every state, deliberate behaviour, values not
   wording), say whether the change touches it. For the ones it touches,
   check that it holds: reload the page, restart the server while the page
   is open, and drive the states with the control API (for example
   `POST /api/shot/start`, then `POST /api/shot/result` or
   `POST /api/shot/abort`).
5. If texts changed, switch to German under Status → Settings → Language
   and check the same screens; `cargo test` checks the catalogs' keys, not
   whether the words fit.
6. Make sure nothing scrolls sideways and the content stays within 480 px.
7. Report what was checked, with screenshots of the changed screens in light
   and dark, and anything that was not checked. Reset the viewport when
   done.
