# winget package for DesktopShepherd

## What winget is
winget is Windows 11's built-in app installer. Once DesktopShepherd is in its catalogue, anyone
can type `winget install radioactivetoy.DesktopShepherd` and get it installed (and updated with
`winget upgrade`) without visiting GitHub. The catalogue is a public GitHub project,
`microsoft/winget-pkgs`, that holds one small text "manifest" per app version.

## The files here
- `radioactivetoy.DesktopShepherd.yaml` - the version file.
- `radioactivetoy.DesktopShepherd.installer.yaml` - where to download the setup, its SHA-256
  fingerprint, and the silent switch (`--silent`).
- `radioactivetoy.DesktopShepherd.locale.en-US.yaml` - name, description, licence, links.
- `make-manifest.ps1` - fills in version, download link and fingerprint for a release.

The files committed here describe v1.0.0-rc.2 as a sample. **Do not submit them as they are:**
rc.2's setup has no silent mode, so winget could not install it without showing windows.
Submit the FIRST release that contains `--silent` (the next one). After publishing that release:

    .\packaging\winget\make-manifest.ps1 -Tag v1.0.1

(use the real tag). It reads that release's `SHA256SUMS.txt`, and writes the three finished
files to `packaging\winget\out\<version>\`. Nothing is uploaded.

## Test it on this PC first
1. `winget validate --manifest packaging\winget\out\<version>` - checks the files' format.
2. To try a real install from the local files, winget must allow local manifests. In an
   **administrator** PowerShell run `winget settings --enable LocalManifestFiles` (one time).
3. `winget install --manifest packaging\winget\out\<version>` - downloads the setup from GitHub,
   checks the fingerprint and installs silently. Then `winget uninstall radioactivetoy.DesktopShepherd`.
   (Close the running DesktopShepherd first only if you want to see the "asks it to quit" step
   work; the setup asks it to quit by itself.) Turn the setting off again with
   `winget settings --disable LocalManifestFiles`.

Silent mode by hand: `DesktopShepherd-Setup-<version>.exe --silent` installs (or updates) with no
windows and exit code 0 (non-zero = failed; details in `%APPDATA%\DesktopShepherd\log.txt`). It does not start
the program afterwards. `DesktopShepherd.exe --uninstall --silent` removes it and keeps your boxes.

## Submitting (needs you; nothing here does it)
1. Have a GitHub account (yours) and fork `microsoft/winget-pkgs`.
2. Copy the three files into your fork at
   `manifests/r/radioactivetoy/DesktopShepherd/<version>/`.
3. Open a pull request. Microsoft's bots check the files, download and test-install the setup,
   and scan it for viruses. If a check fails, the bot comments what to fix.
4. A moderator merges it, usually within days. After that `winget install
   radioactivetoy.DesktopShepherd` works. (Setups that are not code-signed can be flagged by
   SmartScreen/antivirus checks; if that happens the bot says so in the pull request.)

The easy way to do steps 1-3: install `wingetcreate` (`winget install wingetcreate`) and run
`wingetcreate submit packaging\winget\out\<version>` - it forks, copies and opens the pull request
using a GitHub token you give it.

## Later releases (optional automation)
Each release needs a new pull request with the new version, link and fingerprint. This can be
automatic: add a step to `.github/workflows/release.yml` that, after the release is published,
runs `wingetcreate update radioactivetoy.DesktopShepherd --version <version> --urls <setup url>
--submit --token ${{ secrets.WINGET_TOKEN }}`. For that you create a GitHub personal access token
(classic, scope `public_repo`) and save it in this repository's Settings > Secrets and variables >
Actions as `WINGET_TOKEN`. This is not set up; do it only when you want it.
