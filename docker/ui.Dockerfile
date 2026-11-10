# Toolchain image of the `ui` area (local runs and CI use the same image; T-064, decision #89).
# Every `scripts/tw-run ui -- ...` command runs in it: lint, unit tests, licenses and the e2e
# run, whose Playwright runner, webServer and Chromium so get a network namespace of their own
# (docs/decisions/ui-e2e.md E3). Build: `make ui-image`, which takes the tag from the ui area's
# `image` in the teamwright config (scripts/ci/area-image.sh, T-177) and passes
# PLAYWRIGHT_VERSION and VOICEN_UI_IMAGE from it.
# Base: the official Playwright image at the lockfile's @playwright/test version (Ubuntu 24.04,
# Node 24, browsers in /ms-playwright, /ms-playwright/.docker-info with its driverVersion,
# which the e2e entry scripts/e2e.sh compares with node_modules/@playwright/test), pinned by
# the digest of its manifest list (`docker buildx imagetools inspect
# mcr.microsoft.com/playwright:v<version>-noble`). Bump: @playwright/test in pnpm-lock.yaml,
# the ui area's image in the config, and the PLAYWRIGHT_DIGEST default below. The digest wins
# over the tag: a stale digest builds the old base, and scripts/e2e.sh's driverVersion check
# (exit 3) stops the next run.
ARG PLAYWRIGHT_VERSION
ARG PLAYWRIGHT_DIGEST=eff16c30e6f3f4af0a03fa4b706120d5e9b0891c344a27d64559aff5900a4a27
FROM mcr.microsoft.com/playwright:v${PLAYWRIGHT_VERSION}-noble@sha256:${PLAYWRIGHT_DIGEST}
# pnpm at package.json packageManager, baked in: the official image has none, and tw-run's
# containers are removed after each command, so corepack would download it on every run.
RUN npm install -g pnpm@8.15.0 \
 && pnpm --version \
 && npm cache clean --force
# The image's own marker: playwright.config.ts starts only when this is set AND a container
# marker file (/.dockerenv, /run/.containerenv) exists, so Playwright never runs on the host
# (T-064 review 1 #2). The value is the tag (make ui-image passes it).
ARG VOICEN_UI_IMAGE
ENV VOICEN_UI_IMAGE=$VOICEN_UI_IMAGE
