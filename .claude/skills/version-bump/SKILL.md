---
name: version-bump
description: Prepare a Hydra release by opening a version-bump pull request. Use when the user asks to bump the version, cut or prepare a release, or ship what is on main. It picks major, minor or patch from what merged since the last release, edits the workspace version, and opens a PR whose merge tags vX.Y.Z and builds a draft release.
---

# Version bump

Hydra has one version for the whole workspace: `version` under `[workspace.package]` in the root `Cargo.toml`. Every crate inherits it, the installers are named after it, and the app's update check compares against it.

A release is a pull request that changes only that version (and `Cargo.lock`). Nothing else rides along. When it merges:
1. `.github/workflows/main.yml` tags the merge commit `vX.Y.Z`.
2. It calls `.github/workflows/release.yml`, which builds the Windows MSI and the macOS DMG.
3. That workflow opens a **draft** GitHub release.

The maintainer runs `docs/pilot.md`'s release checklist on the draft before publishing it. Never push tags, create releases or publish drafts yourself: merging the PR is the human approval, and the workflows do the rest.

## Which number goes up

Versions are `MAJOR.MINOR.PATCH`.

- **Major: only when the user explicitly asks for it.** Never infer a major bump, even for a breaking change. If you see one, say so and ask (see step 3).
- **Minor: a feature release.** At least one `feat:` PR merged since the last release. Reset patch to 0: `0.3.2` becomes `0.4.0`.
- **Patch: everything else.** Only `fix:`, `docs:`, `test:`, `chore:`, `ci:`, `refactor:` or `perf:` merged: `0.3.2` becomes `0.3.3`.
- **Nothing merged since the last tag: no release.** Tell the user and stop.

## Steps

1. **Start from an up-to-date main, in a clean tree.**
   ```sh
   git fetch origin --tags
   git status --short   # must be empty
   ```
2. **Find the last release and what merged since.**
   ```sh
   last=$(git tag -l 'v*' --sort=-v:refname | head -1)
   git log --first-parent --format='%s' "${last:-$(git rev-list --max-parents=0 origin/main)}"..origin/main
   ```
   Classify each subject by its prefix. Merge commits read "Merge pull request #N from …"; look up each PR's title with `gh pr view N --json title -q .title` and classify that.
   - No tag yet: the first release is the current version as written (`0.1.0`). Confirm that with the user rather than bumping.
   - Untyped subjects: read the PR or diff and decide whether it was a feature or a fix. If unsure, ask.
3. **Look for breaking changes and things the release notes must call out.**
   - Breaking changes: a `!` after the type (`feat!:`), "BREAKING CHANGE" in a body, or anything that stops older files or databases from working.
   - New migrations: `git diff --name-only "$last"..origin/main -- crates/hydra-db/migrations`. A new migration means older apps refuse the upgraded database, so the app and `hydra-mcp` must be updated together. That is expected, not breaking.
   - A higher `SCHEMA_VERSION` in `crates/hydra-core/src/circuit.rs`: newer circuit files can't be opened by older apps.
   - If something looks breaking, tell the user and ask whether they want a major bump. Bump major only if they say so.
4. **Tell the user the plan before changing anything:** "Last release vA.B.C; since then N features, M fixes; proposing vX.Y.Z", with the notable items from step 3. Go ahead once they agree, or if they already named the bump.
5. **Make the change on its own branch.**
   ```sh
   git switch -c release/vX.Y.Z origin/main
   # edit Cargo.toml: [workspace.package] version = "X.Y.Z"
   cargo check --workspace     # refreshes Cargo.lock: only the workspace crates' versions change
   git diff --stat             # exactly Cargo.toml and Cargo.lock
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   ```
6. **Commit, push and open the PR.** No AI attribution in either: no `Co-Authored-By`, no "Generated with" line.
   ```sh
   git commit -am "chore: release vX.Y.Z"
   git push -u origin release/vX.Y.Z
   gh pr create --base main --title "chore: release vX.Y.Z" --body-file <notes>
   ```
   The PR body is the release summary, grouped:
   - **Features**: one line each, with the PR number.
   - **Fixes**
   - **Other**
   - **Upgrade notes**: new migrations, a schema bump, anything from step 3. Write "None" if empty.

   End the body with: "Merging this tags vX.Y.Z and builds a draft release; publish it after the pilot checklist (docs/pilot.md)."
7. **Report the PR link and the chosen version.** The PR checks include a `version` job that fails if the version goes down or the tag already exists.

## If something goes wrong

- **The version check fails:** someone released meanwhile. Rebase on `origin/main` and recompute from step 2.
- **The release workflow fails after the merge:**
  - the tag already exists, so don't bump again to retry;
  - fix the workflow in a normal PR, then rerun the failed release run from the Actions tab;
  - or ask the maintainer to delete the tag and the draft, then rerun the Main workflow on the merge commit.
- **The user wants to release a fix on top of an older release:** that needs a release branch, which this flow doesn't cover. Ask the user how they want it done.
