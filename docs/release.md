# Release

## Before switching this repository to private

This repository stays public during development and becomes private only
right before the final release. The GPL-3.0 driver stays public in
[lightsound/noican-driver](https://github.com/lightsound/noican-driver)
(docs/driver.md). Every item must hold before the owner changes the
visibility (Settings → General → Danger Zone → Change repository
visibility); the switch itself is the last step.

1. **Driver source is public and tagged.**
   `lightsound/noican-driver` is public, its CI is green, and the shipped
   driver is tagged `driver-v<version>` there (`<version>` = the bundle's
   `CFBundleShortVersionString`; the driver CI fails a tag that does not
   match). The gitlink here points at that tag:
   `git -C external/noican-driver describe --tags --exact-match` prints it.
2. **Every source offer points at the public repository.** The shipped
   bundle's notice names it:
   `grep -F https://github.com/lightsound/noican-driver
   external/noican-driver/dist/Noican.driver/Contents/Resources/LICENSE`.
   Open that URL signed out and confirm it resolves. Nothing public
   (driver repository, sales page, release notes, EULA) links to
   `github.com/lightsound/noican` as a place to get source:
   `git -C external/noican-driver grep -nE 'lightsound/noican([^-]|$)'`
   prints nothing.
3. **CI has a runner plan.** Private repositories draw on the Actions
   quota (GitHub Free: 2,000 min/month, macOS minutes count 10×); the
   macOS job alone exceeds it at the current push rate. Pick one before
   switching: a self-hosted runner on the owner's Mac (Settings → Actions
   → Runners, then `runs-on: [self-hosted, macOS, ARM64]` for the macOS
   job — self-hosted runners only on a private repository), a paid plan
   or spending limit, or running the macOS job only on `main` and release
   tags. The Linux jobs also count against the quota (1×).
4. **CI still checks out the driver.** The submodule URL is the public
   HTTPS URL, so `submodules: recursive` needs no token; confirm the first
   CI run after the switch passes the macOS job.
5. **Integrations keep access.** Grant the Cursor, Pullfrog, and any other
   GitHub App access to the private repository (Settings → Applications →
   Configure → Repository access) and confirm one agent run can clone
   and push.
6. **No public copies remain attached.** The fork count is 0 (a public
   fork stays public after the switch), and GitHub Pages is off.
