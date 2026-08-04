---
allowed-tools: Bash, Edit, Read, Glob
argument-hint: [version] (e.g., 0.4.0)
description: Automated release process - version bump, tag, publish binaries and crate
---

# Automated Release Process

Execute the complete release workflow for pai-sho project.

## Pre-flight Checks

Current repository status: !`git status`

Current branch: !`git branch --show-current`

Last few releases: !`git tag --sort=-version:refname | head -5`

Current version: !`grep '^version' Cargo.toml | head -1`

## Release Steps

### 1. Pre-Release Information Gathering

**Ask the user for the following BEFORE starting anything else:**

- Confirm the version number: $ARGUMENTS
- Crates.io credentials, unless already present. Verify with
  `cargo publish --dry-run` reaching the "aborting upload due to dry run"
  stage without a token error; if missing, have the user run `cargo login`
  (via `! cargo login` in the session) or provide a token for
  `CARGO_REGISTRY_TOKEN`. Collecting this now means the release cannot
  strand at the publish step hours later.

### 2. Version Management

- Update version in Cargo.toml to $ARGUMENTS
- Run `cargo check` to update Cargo.lock

### 3. Generate Changelog

Get commits since last stable release:

```bash
last_tag=$(git tag --list 'v*' --sort=-version:refname | head -1)
git log --oneline --pretty=format:"* %s (%ad)" --date=short ${last_tag}..HEAD
```

Create `changes/v$ARGUMENTS.md` with:

- `# v$ARGUMENTS` header
- `## Highlights` section with notable user-facing changes
- `## Raw commits` section with the commit list
- **No soft line breaks** -- paragraphs should be single long lines, not
  wrapped at 80 columns. GitHub renders markdown with soft wraps, so hard
  breaks mid-paragraph show up as unwanted newlines in the release notes.

### 4. Review Release Notes

**WARNING: REVIEW REQUIRED**: Show the changelog for user approval before
proceeding:

- Check that all important changes are highlighted appropriately
- Edit the highlights section to focus on user-facing improvements
- Ensure the changelog is accurate and complete

**Do not proceed to the next step until the user is satisfied with the
release notes.**

### 5. Git Operations

- Commit changes with message: `chore: release v$ARGUMENTS`
- Create and push git tag `v$ARGUMENTS`:
  ```bash
  git tag v$ARGUMENTS
  git push origin main
  git push origin v$ARGUMENTS
  ```
- This triggers GitHub workflow to build cross-platform binaries

### 6. Watch CI Workflow

- Get the latest workflow run ID: `gh run list --limit 1`
- Monitor build with: `gh run watch <run-id> --exit-status`
- Wait for all three builds to complete (linux-amd64, linux-arm64, macos-arm64)

### 7. Verify Release Artifacts

- Verify GitHub release: `gh release view v$ARGUMENTS`
- Ensure all artifacts are uploaded:
  - pai-sho-v$ARGUMENTS-linux-amd64.tar.gz
  - pai-sho-v$ARGUMENTS-linux-arm64.tar.gz
  - pai-sho-v$ARGUMENTS-macos-arm64.tar.gz
- Verify release notes: `gh release view v$ARGUMENTS --json body`
- If release body is just the commit message, update it:
  ```bash
  gh release edit v$ARGUMENTS --notes-file changes/v$ARGUMENTS.md
  ```

### 8. Test Installation

**Verify eget installation works:**

```bash
eget cablehead/pai-sho --tag v$ARGUMENTS
```

If eget fails, check:
- Artifact naming matches eget conventions
- Tarball structure is correct (binary in top-level directory)

### 9. Homebrew Formula Update

- Clone `../homebrew-tap` if not present:
  `git clone https://github.com/cablehead/homebrew-tap.git`
- **Pull latest** before making changes: `cd ../homebrew-tap && git pull`
- **Wait 10+ seconds** after build completes for GitHub CDN propagation
- Download macOS tarball, verify integrity, and calculate SHA256:
  ```bash
  cd /tmp
  rm -f pai-sho-v$ARGUMENTS-macos-arm64.tar.gz
  curl -sL https://github.com/cablehead/pai-sho/releases/download/v$ARGUMENTS/pai-sho-v$ARGUMENTS-macos-arm64.tar.gz -o pai-sho-v$ARGUMENTS-macos-arm64.tar.gz
  tar -tzf pai-sho-v$ARGUMENTS-macos-arm64.tar.gz  # verify before hashing
  sha256sum pai-sho-v$ARGUMENTS-macos-arm64.tar.gz
  ```
- Update `../homebrew-tap/Formula/pai-sho.rb` with new version, URL, and
  SHA256 checksum
- Commit and push homebrew formula changes

### 10. Manual Verification Required

**WARNING: CRITICAL: macOS Verification BEFORE Publishing to Crates.io**

After homebrew formula is updated, **PAUSE** and ask a macOS user to test:

```bash
brew uninstall pai-sho  # if previously installed
brew install cablehead/tap/pai-sho
pai-sho --version  # should show $ARGUMENTS
```

**STOP HERE if verification fails.** Publishing to crates.io is irreversible.

### 11. Cargo Registry Publication

**Only proceed after macOS verification passes.**

Using the token collected in step 1:

```bash
cargo publish
```

**Warning**: This step cannot be undone - you cannot unpublish from crates.io

### 12. Bump to Dev Version

After publishing, bump Cargo.toml to the next patch dev version
(e.g., `0.4.0` -> `0.4.1-dev`), run `cargo check` to update Cargo.lock,
and commit:

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: bump to v<next>-dev"
git push
```

## Release Complete

The release is now public! Summary:
- GitHub release: https://github.com/cablehead/pai-sho/releases/tag/v$ARGUMENTS
- eget: `eget cablehead/pai-sho`
- Homebrew: `brew install cablehead/tap/pai-sho`
- Crates.io: `cargo install pai-sho`

## Rollback Plan

If verification fails **before cargo publish**:

1. Delete the git tag:
   ```bash
   git tag -d v$ARGUMENTS
   git push --delete origin v$ARGUMENTS
   ```
2. Delete the GitHub release:
   ```bash
   gh release delete v$ARGUMENTS --yes
   ```
3. Revert homebrew formula changes
4. Revert version changes in Cargo.toml
5. Investigate and fix issues before retry

**Note**: If cargo publish has already completed, you cannot unpublish from
crates.io. You would need to publish a new patch version with the fix instead.

---

**Ready to execute release for version $ARGUMENTS?**
