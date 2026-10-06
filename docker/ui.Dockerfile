# Toolchain image of the `ui` area (local runs and CI use the same image; T-064, decision #89).
# Every `scripts/tw-run ui -- ...` command runs in it: lint, unit tests, licenses and the e2e
# run, whose Playwright runner, webServer and Chromium so get a network namespace of their own
# (docs/decisions/ui-e2e.md E3). Build: `make ui-image` (tag voicen-ui:<Playwright version>).
# Base: the official Playwright image at the lockfile's @playwright/test version (Ubuntu 24.04,
# Node 24, browsers in /ms-playwright, /ms-playwright/.docker-info with its driverVersion,
# which the e2e entry scripts/e2e.sh compares with node_modules/@playwright/test).
# Bump the tag together with @playwright/test in pnpm-lock.yaml, the Makefile ui-image tag and
# the ui area's image in .teamwright/config.yml.
FROM mcr.microsoft.com/playwright:v1.63.0-noble
# pnpm at package.json packageManager, baked in: the official image has none, and tw-run's
# containers are removed after each command, so corepack would download it on every run.
RUN npm install -g pnpm@8.15.0 \
 && pnpm --version \
 && npm cache clean --force
