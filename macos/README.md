# Nitpick Agent macOS App

This directory contains the thin Swift menu bar shell for `Nitpick Agent.app`.

The app owns macOS concerns only:

- status bar menu
- launching and stopping `nitpick-agent-host`
- Sparkle update checks
- packaging Rust binaries into an `.app` bundle

The review/chat runtime stays in `nitpick-agent-core`.

The menu bar app should read activity/artifact status from `nitpick-agent-host`. It should not sync directly to GitHub; sync is a separate destination adapter over the local artifact store.

## Sparkle Configuration

`Bundle/Info.plist` is configured for Sparkle updates from:

- `SUFeedURL`: `https://github.com/stephanos/nitpick-agent/releases/latest/download/appcast.xml`
- `SUPublicEDKey`: public key for the local `nitpick-agent` Sparkle signing account

The private EdDSA key must stay out of the repository. On a release machine,
store it in Keychain under Sparkle account `nitpick-agent`:

```bash
macos/.build/artifacts/sparkle/Sparkle/bin/generate_keys --account nitpick-agent
```

For CI, store the private key as the GitHub repository secret
`SPARKLE_PRIVATE_ED_KEY`; the release workflow pipes it to Sparkle with
`--ed-key-file -`.

Build the app and generate the signed appcast archive with:

```bash
CODESIGN_IDENTITY="Developer ID Application: ..." mise run macos-appcast
```

For local smoke tests, ad-hoc signing can exercise the Sparkle archive path:

```bash
CODESIGN_IDENTITY="-" mise run macos-appcast
```

Sparkle rejects unsigned update archives, so `CODESIGN_IDENTITY` is required.

The script writes `target/sparkle/Nitpick-Agent-<version>-<build>.zip` and
`target/sparkle/appcast.xml`. Upload both files to the GitHub release that
should be served by the `latest/download` feed URL.
