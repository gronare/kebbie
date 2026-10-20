# Release kelpie

For maintainers. A release happens only when you push a tag. CI runs the tests on every push and releases nothing.

1. Bump the version in `Cargo.toml`, `herdr-plugin.toml` and `.claude-plugin/plugin.json`. A test fails when the three differ. Run `cargo build` so `Cargo.lock` follows.

2. Commit and push to main:

   ```bash
   git commit -am "kelpie 1.4.0" && git push
   ```

   Machines on the `main` channel can try it from here.

3. Tag that commit and push the tag:

   ```bash
   git tag v1.4.0 && git push origin v1.4.0
   ```

   Machines on the `release` channel show `↑ kelpie v1.4.0` within 6 hours, and `kelpie update` installs it.

4. Publish the draft. The tag runs `.github/workflows/release.yml`. It fails when the tag and the three manifests disagree, runs the tests, and drafts a GitHub Release with notes generated from the commits since the previous tag. Review the notes and publish the draft yourself.

A release tag is exactly `v` and three numbers. `v1.4`, `v1.4.0-rc1` and other tags are never offered as releases.
