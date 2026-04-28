# Social Video Studio Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Social Video Studio as a separate local Remotion/React/Node app that creates 10 daily short-form video posting packs with SNS-specific comments and manual posting workflow.

**Architecture:** The implementation lives outside ZWG Terminal at `D:/NEXTCLOUD/Windows_app/SocialVideoStudio`. A Node/TypeScript CLI runs scheduled generation without the API server, writes SQLite and `exports/YYYY-MM-DD/`, and the React dashboard reads the same state through a local API. Remotion owns only video composition/rendering, while collectors, scoring, AI generation, export, and scheduling are separate modules.

**Tech Stack:** TypeScript, React, Vite, Remotion, Node CLI, Express, SQLite, Vitest, Playwright, Windows Task Scheduler. Use @everything-claude-code:remotion-best-practices for video work, @superpowers:test-driven-development for implementation, @everything-claude-code:security-review for X.com/browser/secret handling, and @everything-claude-code:e2e-testing for dashboard verification.

---

## Source Documents

- Spec: `D:/NEXTCLOUD/Windows_app/ZWG_Terminal/docs/superpowers/specs/2026-04-29-social-video-studio-design.md`
- Plan: `D:/NEXTCLOUD/Windows_app/ZWG_Terminal/docs/superpowers/plans/2026-04-29-social-video-studio-implementation.md`
- Implementation root: `D:/NEXTCLOUD/Windows_app/SocialVideoStudio`

## Scope

This plan builds the Phase 1 and Phase 2 MVP described in the spec:

- Independent project outside ZWG Terminal.
- Daily generation CLI.
- RSS/API, manual override, official-page/browser collectors, and safe X.com readonly collector.
- Category minimums plus trend scoring.
- AI provider abstraction with fake, OpenAI, and local-LLM stubs.
- No AI voice.
- Remotion 9:16 videos with Mac-style briefing and product-card variants.
- SQLite source of truth.
- Date folder export.
- Local dashboard for manual posting.
- Windows Task Scheduler install command.

Phase 3 capabilities are planned as extension points only in this pass:

- SNS API auto-posting or reservation.
- Deep ZWG Terminal integration.
- Full analytics based on external platform metrics.
- Media-specific video variants beyond shared 9:16 output.

## File Structure

Create these files under `D:/NEXTCLOUD/Windows_app/SocialVideoStudio`.

### Root

- `package.json` - scripts and dependencies.
- `tsconfig.json` - shared TypeScript settings.
- `vite.config.ts` - dashboard build config.
- `vitest.config.ts` - unit/integration test config.
- `playwright.config.ts` - dashboard E2E config.
- `remotion.config.ts` - Remotion config.
- `.env.example` - safe config template, no secrets.
- `.gitignore` - exclude `.env`, SQLite DBs, exports, logs, node_modules.
- `README.md` - setup, run, schedule, safety notes.

### Shared Domain

- `src/shared/types.ts` - canonical domain models.
- `src/shared/categories.ts` - category definitions and defaults.
- `src/shared/status.ts` - generation/posting/review states.
- `src/shared/sns.ts` - SNS targets, limits, and comment shape.
- `src/shared/paths.ts` - deterministic app, DB, log, and export paths.
- `src/shared/validation.ts` - Zod schemas for runtime validation.

### Server and CLI

- `src/server/cli.ts` - CLI entrypoint and command routing.
- `src/server/config/env.ts` - `.env` and `--env-file` loading.
- `src/server/config/settings.ts` - user settings load/save.
- `src/server/logging/logger.ts` - redacted logging.
- `src/server/db/client.ts` - SQLite connection.
- `src/server/db/schema.sql` - schema source.
- `src/server/db/migrate.ts` - idempotent migration runner.
- `src/server/db/repositories/*.ts` - focused repositories.
- `src/server/jobs/generateToday.ts` - CLI job orchestration.
- `src/server/jobs/regenerate.ts` - comment/video/candidate regeneration.
- `src/server/api/server.ts` - Express app factory.
- `src/server/api/routes/*.ts` - dashboard API routes.

### Collection and Selection

- `src/server/collectors/types.ts` - collector interface.
- `src/server/collectors/policy.ts` - robots/rate/safety policy.
- `src/server/collectors/manual.ts` - manual override collector.
- `src/server/collectors/rss.ts` - RSS/API collector.
- `src/server/collectors/officialPage.ts` - official page collector.
- `src/server/collectors/browser.ts` - browser collector wrapper.
- `src/server/collectors/xReadonly.ts` - X.com readonly collector.
- `src/server/collectors/index.ts` - collector registry.
- `src/server/scoring/scorer.ts` - freshness/trend/trust scoring.
- `src/server/scoring/selection.ts` - category minimum selection.
- `src/server/scoring/deduplicate.ts` - duplicate detection.
- `src/server/safety/ngWords.ts` - NG word checks.
- `src/server/safety/claims.ts` - high-risk claim checks.
- `src/server/media/imagePolicy.ts` - image metadata and embed decisions.

### AI and Export

- `src/server/ai/types.ts` - provider interface.
- `src/server/ai/fakeProvider.ts` - deterministic provider for tests.
- `src/server/ai/openaiProvider.ts` - OpenAI implementation.
- `src/server/ai/localProvider.ts` - local LLM implementation stub.
- `src/server/ai/prompts.ts` - prompt builders.
- `src/server/export/manifest.ts` - JSON manifest creation.
- `src/server/export/postText.ts` - SNS text files.
- `src/server/export/csv.ts` - CSV summary.
- `src/server/export/files.ts` - date folder writer.

### Remotion

- `src/remotion/index.ts` - Remotion entry.
- `src/remotion/Root.tsx` - compositions.
- `src/remotion/schema.ts` - composition props schema.
- `src/remotion/templates/BriefingVideo.tsx` - Mac-style briefing video.
- `src/remotion/templates/ProductCardVideo.tsx` - product-card video.
- `src/remotion/components/*.tsx` - shared typography, source badge, safe text.
- `src/remotion/render/renderVideo.ts` - server-side render wrapper.
- `src/remotion/render/renderStill.ts` - still verification wrapper.

### Dashboard

- `src/dashboard/main.tsx` - Vite entry.
- `src/dashboard/App.tsx` - shell, routing, layout.
- `src/dashboard/api/client.ts` - typed API client.
- `src/dashboard/pages/TodayPage.tsx` - daily pack overview.
- `src/dashboard/pages/DetailPage.tsx` - post pack detail.
- `src/dashboard/pages/SettingsPage.tsx` - settings.
- `src/dashboard/pages/AnalyticsPage.tsx` - history summaries.
- `src/dashboard/components/*.tsx` - reusable UI controls.
- `src/dashboard/styles.css` - Mac-like operational UI.

### Scripts and Tests

- `scripts/install-schedule.ps1` - Windows Task Scheduler install.
- `scripts/run-generate.ps1` - scheduler target wrapper.
- `tests/unit/**/*.test.ts` - pure unit tests.
- `tests/integration/**/*.test.ts` - SQLite, export, job integration.
- `tests/e2e/dashboard.spec.ts` - Playwright dashboard flow.
- `tests/fixtures/manual-overrides.json` - deterministic manual items.
- `tests/fixtures/rss/*.xml` - deterministic RSS feeds.
- `tests/fixtures/images/*` - safe local test images.

## Task 1: Scaffold Independent Project

**Files:**
- Create: `D:/NEXTCLOUD/Windows_app/SocialVideoStudio/package.json`
- Create: `D:/NEXTCLOUD/Windows_app/SocialVideoStudio/tsconfig.json`
- Create: `D:/NEXTCLOUD/Windows_app/SocialVideoStudio/vite.config.ts`
- Create: `D:/NEXTCLOUD/Windows_app/SocialVideoStudio/vitest.config.ts`
- Create: `D:/NEXTCLOUD/Windows_app/SocialVideoStudio/remotion.config.ts`
- Create: `D:/NEXTCLOUD/Windows_app/SocialVideoStudio/.gitignore`
- Create: `D:/NEXTCLOUD/Windows_app/SocialVideoStudio/.env.example`
- Create: `D:/NEXTCLOUD/Windows_app/SocialVideoStudio/README.md`

- [ ] **Step 1: Create the independent folder**

Run:

```powershell
New-Item -ItemType Directory -Force -Path 'D:\NEXTCLOUD\Windows_app\SocialVideoStudio'
```

Expected: directory exists outside `D:\NEXTCLOUD\Windows_app\ZWG_Terminal`.

- [ ] **Step 2: Initialize git**

Run:

```powershell
Set-Location 'D:\NEXTCLOUD\Windows_app\SocialVideoStudio'
git init
```

Expected: new independent Git repository.

- [ ] **Step 3: Create `package.json`**

Use npm scripts:

```json
{
  "name": "social-video-studio",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite --host 127.0.0.1",
    "api": "tsx src/server/api/server.ts",
    "serve": "tsx src/server/cli.ts serve",
    "generate:today": "tsx src/server/cli.ts generate-today",
    "schedule:run": "powershell -ExecutionPolicy Bypass -File scripts/run-generate.ps1",
    "schedule:install": "powershell -ExecutionPolicy Bypass -File scripts/install-schedule.ps1",
    "remotion:studio": "remotion studio src/remotion/index.ts",
    "remotion:still": "remotion still src/remotion/index.ts BriefingVideo out/still.png --frame=30 --scale=0.25",
    "remotion:sample": "remotion render src/remotion/index.ts BriefingVideo out/sample.mp4 --frames=0-90 --scale=0.25",
    "test": "vitest run",
    "test:watch": "vitest",
    "test:e2e": "playwright test",
    "typecheck": "tsc --noEmit",
    "build": "vite build && tsc --noEmit"
  }
}
```

- [ ] **Step 4: Install dependencies**

Run:

```powershell
npm install @vitejs/plugin-react vite react react-dom lucide-react express zod dotenv better-sqlite3 rss-parser openai remotion @remotion/cli @remotion/renderer @remotion/player
npm install -D typescript tsx vitest @testing-library/react @testing-library/jest-dom jsdom playwright @playwright/test @types/node @types/express @types/better-sqlite3
```

Expected: `package-lock.json` created.

- [ ] **Step 5: Add `.gitignore`**

Required entries:

```gitignore
node_modules/
dist/
out/
.env
*.db
*.db-shm
*.db-wal
exports/
logs/
.playwright/
test-results/
```

- [ ] **Step 6: Add `.env.example`**

Required values:

```dotenv
AI_PROVIDER=fake
OPENAI_API_KEY=
OPENAI_MODEL=
LOCAL_LLM_ENDPOINT=http://127.0.0.1:11434
DATABASE_PATH=./data/social-video-studio.db
EXPORT_ROOT=./exports
LOG_ROOT=./logs
ENABLE_BROWSER_COLLECTORS=false
ENABLE_X_READONLY=false
```

- [ ] **Step 7: Run initial checks**

Run:

```powershell
npm run typecheck
npm test
```

Expected: typecheck may pass with empty project; tests report no tests or pass after adding placeholder config.

- [ ] **Step 8: Commit**

```powershell
git add .
git commit -m "chore: scaffold social video studio"
```

## Task 2: Define Domain Models and Validation

**Files:**
- Create: `src/shared/types.ts`
- Create: `src/shared/categories.ts`
- Create: `src/shared/status.ts`
- Create: `src/shared/sns.ts`
- Create: `src/shared/paths.ts`
- Create: `src/shared/validation.ts`
- Test: `tests/unit/shared/domain.test.ts`

- [ ] **Step 1: Write failing domain tests**

Test examples:

```ts
import { categories, defaultCategoryMinimums } from "../../../src/shared/categories";
import { snsTargets } from "../../../src/shared/sns";
import { candidateSchema } from "../../../src/shared/validation";

test("defines the approved initial categories", () => {
  expect(categories.map((category) => category.id)).toEqual([
    "ai_llm",
    "gadgets",
    "domestic_news",
    "x_trends",
    "hundred_yen",
    "convenience_new_items",
  ]);
});

test("keeps all four SNS targets", () => {
  expect(snsTargets.map((target) => target.id)).toEqual(["x", "instagram", "tiktok", "youtube_shorts"]);
});

test("validates candidate shape", () => {
  const candidate = candidateSchema.parse({
    id: "candidate-1",
    title: "AI model update",
    categoryId: "ai_llm",
    sourceUrl: "https://example.com/news",
    sourceName: "Example",
    publishedAt: "2026-04-29T06:00:00.000Z",
    collectedAt: "2026-04-29T06:01:00.000Z",
    excerpt: "Short summary",
    imageCandidates: [],
  });
  expect(candidate.categoryId).toBe("ai_llm");
});
```

- [ ] **Step 2: Run the failing test**

Run:

```powershell
npm test -- tests/unit/shared/domain.test.ts
```

Expected: FAIL because shared modules do not exist.

- [ ] **Step 3: Implement domain modules**

Key type shape:

```ts
export type CategoryId =
  | "ai_llm"
  | "gadgets"
  | "domestic_news"
  | "x_trends"
  | "hundred_yen"
  | "convenience_new_items";

export type SnsTargetId = "x" | "instagram" | "tiktok" | "youtube_shorts";

export type LicenseStatus = "allowed" | "unknown" | "restricted";
export type UsageDecision = "embed" | "metadata-only" | "reject";

export type ImageCandidate = {
  sourceUrl: string;
  sourcePublisher: string;
  licenseStatus: LicenseStatus;
  usageDecision: UsageDecision;
  requiresReview: boolean;
  downloadedAt?: string;
  localPath?: string;
};
```

- [ ] **Step 4: Re-run unit tests**

Run:

```powershell
npm test -- tests/unit/shared/domain.test.ts
```

Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add src/shared tests/unit/shared
git commit -m "feat: define social video domain model"
```

## Task 3: Add Config, Paths, and Redacted Logging

**Files:**
- Create: `src/server/config/env.ts`
- Create: `src/server/config/settings.ts`
- Create: `src/server/logging/logger.ts`
- Test: `tests/unit/server/config.test.ts`
- Test: `tests/unit/server/logger.test.ts`

- [ ] **Step 1: Write failing config tests**

Cover:

- Reads `.env` from project root by default.
- Reads explicit `--env-file`.
- Requires `OPENAI_API_KEY` only when `AI_PROVIDER=openai`.
- Does not require secrets when `AI_PROVIDER=fake` or `local`.
- Resolves `DATABASE_PATH`, `EXPORT_ROOT`, and `LOG_ROOT`.

- [ ] **Step 2: Write failing logger tests**

Expected behavior:

```ts
expect(redactSecrets("OPENAI_API_KEY=sk-test")).not.toContain("sk-test");
expect(redactSecrets("cookie=session-value")).not.toContain("session-value");
```

- [ ] **Step 3: Run tests**

```powershell
npm test -- tests/unit/server/config.test.ts tests/unit/server/logger.test.ts
```

Expected: FAIL.

- [ ] **Step 4: Implement config and logging**

Use `dotenv` and `zod`. Keep env parsing pure and testable.

- [ ] **Step 5: Run tests**

```powershell
npm test -- tests/unit/server/config.test.ts tests/unit/server/logger.test.ts
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add src/server/config src/server/logging tests/unit/server
git commit -m "feat: add config and redacted logging"
```

## Task 4: Add SQLite Schema and Repositories

**Files:**
- Create: `src/server/db/schema.sql`
- Create: `src/server/db/client.ts`
- Create: `src/server/db/migrate.ts`
- Create: `src/server/db/repositories/candidates.ts`
- Create: `src/server/db/repositories/packs.ts`
- Create: `src/server/db/repositories/errors.ts`
- Test: `tests/integration/db/schema.test.ts`
- Test: `tests/integration/db/repositories.test.ts`

- [ ] **Step 1: Write failing SQLite migration test**

Test in a temp DB:

```ts
test("migrates a fresh sqlite database", () => {
  const db = openTestDb();
  migrate(db);
  expect(tableNames(db)).toContain("candidates");
  expect(tableNames(db)).toContain("post_packs");
  expect(tableNames(db)).toContain("post_texts");
  expect(tableNames(db)).toContain("job_runs");
});
```

- [ ] **Step 2: Run test**

```powershell
npm test -- tests/integration/db/schema.test.ts
```

Expected: FAIL because DB modules do not exist.

- [ ] **Step 3: Implement schema**

Minimum tables:

- `job_runs`
- `candidates`
- `image_candidates`
- `selected_items`
- `post_packs`
- `post_texts`
- `render_outputs`
- `errors`
- `settings`

- [ ] **Step 4: Implement repositories**

Keep repositories small. Avoid one generic mega-repository.

- [ ] **Step 5: Run integration tests**

```powershell
npm test -- tests/integration/db/schema.test.ts tests/integration/db/repositories.test.ts
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add src/server/db tests/integration/db
git commit -m "feat: add sqlite persistence"
```

## Task 5: Implement Collector Policy, Manual Collector, RSS Collector, and Official Page Collector

**Files:**
- Create: `src/server/collectors/types.ts`
- Create: `src/server/collectors/policy.ts`
- Create: `src/server/collectors/manual.ts`
- Create: `src/server/collectors/rss.ts`
- Create: `src/server/collectors/officialPage.ts`
- Create: `src/server/collectors/index.ts`
- Create: `tests/fixtures/manual-overrides.json`
- Create: `tests/fixtures/rss/ai.xml`
- Create: `tests/fixtures/html/convenience-new-items.html`
- Test: `tests/unit/collectors/policy.test.ts`
- Test: `tests/unit/collectors/manual.test.ts`
- Test: `tests/unit/collectors/rss.test.ts`
- Test: `tests/unit/collectors/officialPage.test.ts`

- [ ] **Step 1: Write failing collector policy tests**

Cover:

- Rejects blocked source policy.
- Marks unknown terms as `requiresReview`.
- Applies per-source once-per-job default.

- [ ] **Step 2: Write failing manual, RSS, and official-page tests**

Manual collector should convert JSON items into canonical candidates. RSS collector should parse fixture XML without network.
Official-page collector should parse a local HTML fixture, apply source policy, extract title, URL, published date when present, and product image metadata without downloading remote images.

- [ ] **Step 3: Run tests**

```powershell
npm test -- tests/unit/collectors
```

Expected: FAIL.

- [ ] **Step 4: Implement collector interface**

Interface:

```ts
export type CollectorResult = {
  sourceId: string;
  candidates: Candidate[];
  errors: CollectorError[];
  skipped: CollectorSkip[];
};

export interface Collector {
  id: string;
  collect(context: CollectorContext): Promise<CollectorResult>;
}
```

- [ ] **Step 5: Implement manual, RSS, and official-page collectors**

Use fixtures in tests. Do not perform live network calls in unit tests.
Official-page collector must not bypass robots.txt or terms policy. It should accept a source config with explicit selectors and return skipped results if the source is not allowed.

- [ ] **Step 6: Run tests**

```powershell
npm test -- tests/unit/collectors
```

Expected: PASS.

- [ ] **Step 7: Commit**

```powershell
git add src/server/collectors tests/fixtures tests/unit/collectors
git commit -m "feat: add source collectors"
```

## Task 6: Implement Scoring, Deduplication, and Selection

**Files:**
- Create: `src/server/scoring/scorer.ts`
- Create: `src/server/scoring/selection.ts`
- Create: `src/server/scoring/deduplicate.ts`
- Test: `tests/unit/scoring/scorer.test.ts`
- Test: `tests/unit/scoring/selection.test.ts`
- Test: `tests/unit/scoring/deduplicate.test.ts`

- [ ] **Step 1: Write failing scoring tests**

Expected:

- Fresh candidates score higher than old candidates.
- Trusted sources score higher than unknown sources.
- `licenseStatus=unknown` lowers trust.
- Missing category candidates do not crash selection.

- [ ] **Step 2: Write failing selection test**

Use 12 candidates across categories and assert:

- Result length is 10.
- Each available category gets at least one item.
- Remaining slots are filled by score.

- [ ] **Step 3: Run tests**

```powershell
npm test -- tests/unit/scoring
```

Expected: FAIL.

- [ ] **Step 4: Implement scoring and selection**

Keep scoring weights in a single exported default config.

- [ ] **Step 5: Run tests**

```powershell
npm test -- tests/unit/scoring
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add src/server/scoring tests/unit/scoring
git commit -m "feat: add candidate scoring and selection"
```

## Task 7: Implement Safety and Image Policy

**Files:**
- Create: `src/server/safety/ngWords.ts`
- Create: `src/server/safety/claims.ts`
- Create: `src/server/media/imagePolicy.ts`
- Test: `tests/unit/safety/ngWords.test.ts`
- Test: `tests/unit/safety/claims.test.ts`
- Test: `tests/unit/media/imagePolicy.test.ts`

- [ ] **Step 1: Write failing safety tests**

Cover:

- NG words mark `requiresReview`.
- Medical, finance, legal claims mark `requiresReview`.
- Unknown or restricted image license returns `usageDecision="metadata-only"` or `reject`.

- [ ] **Step 2: Run tests**

```powershell
npm test -- tests/unit/safety tests/unit/media
```

Expected: FAIL.

- [ ] **Step 3: Implement safety modules**

Avoid deleting candidates. Return review flags with reasons.

- [ ] **Step 4: Run tests**

```powershell
npm test -- tests/unit/safety tests/unit/media
```

Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add src/server/safety src/server/media tests/unit/safety tests/unit/media
git commit -m "feat: add review safety checks"
```

## Task 8: Implement AI Provider Abstraction

**Files:**
- Create: `src/server/ai/types.ts`
- Create: `src/server/ai/fakeProvider.ts`
- Create: `src/server/ai/openaiProvider.ts`
- Create: `src/server/ai/localProvider.ts`
- Create: `src/server/ai/prompts.ts`
- Test: `tests/unit/ai/fakeProvider.test.ts`
- Test: `tests/unit/ai/prompts.test.ts`
- Test: `tests/unit/ai/providerSelection.test.ts`

- [ ] **Step 1: Write failing AI tests**

Fake provider must be deterministic:

```ts
const result = await fakeProvider.generatePostPack(candidate);
expect(result.videoScript.headline).toContain(candidate.title);
expect(result.postTexts.x.text.length).toBeLessThanOrEqual(280);
```

- [ ] **Step 2: Run tests**

```powershell
npm test -- tests/unit/ai
```

Expected: FAIL.

- [ ] **Step 3: Implement provider interface**

Interface:

```ts
export interface AiProvider {
  generatePostPack(input: AiPostPackInput): Promise<AiPostPackResult>;
}
```

- [ ] **Step 4: Implement fake provider and prompt builders**

Use fake provider in tests and default `.env.example`.

- [ ] **Step 5: Implement OpenAI provider behind the same interface**

Do not hardcode model names. Read `OPENAI_MODEL` and require `OPENAI_API_KEY` only when `AI_PROVIDER=openai`.

- [ ] **Step 6: Implement local provider stub**

Call `LOCAL_LLM_ENDPOINT` only when selected. Return clear errors if unavailable.

- [ ] **Step 7: Run tests**

```powershell
npm test -- tests/unit/ai
```

Expected: PASS.

- [ ] **Step 8: Commit**

```powershell
git add src/server/ai tests/unit/ai
git commit -m "feat: add ai provider abstraction"
```

## Task 9: Implement Export Writers

**Files:**
- Create: `src/server/export/manifest.ts`
- Create: `src/server/export/postText.ts`
- Create: `src/server/export/csv.ts`
- Create: `src/server/export/files.ts`
- Test: `tests/unit/export/postText.test.ts`
- Test: `tests/integration/export/files.test.ts`

- [ ] **Step 1: Write failing export tests**

Expected output:

```text
exports/2026-04-29/
  manifest.json
  posts.csv
  01-ai-llm/
    video.mp4
    thumbnail.png
    x.txt
    instagram.txt
    tiktok.txt
    youtube-shorts.txt
```

- [ ] **Step 2: Run tests**

```powershell
npm test -- tests/unit/export tests/integration/export
```

Expected: FAIL.

- [ ] **Step 3: Implement file writers**

Use deterministic slugs and atomic writes where practical.

- [ ] **Step 4: Run tests**

```powershell
npm test -- tests/unit/export tests/integration/export
```

Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add src/server/export tests/unit/export tests/integration/export
git commit -m "feat: add posting pack exports"
```

## Task 10: Implement Remotion Templates

**Files:**
- Create: `src/remotion/index.ts`
- Create: `src/remotion/Root.tsx`
- Create: `src/remotion/schema.ts`
- Create: `src/remotion/templates/BriefingVideo.tsx`
- Create: `src/remotion/templates/ProductCardVideo.tsx`
- Create: `src/remotion/components/SafeText.tsx`
- Create: `src/remotion/components/SourceBadge.tsx`
- Create: `src/remotion/render/renderStill.ts`
- Test: `tests/unit/remotion/schema.test.ts`
- Test: `tests/unit/remotion/templateSelection.test.ts`

- [ ] **Step 1: Read Remotion guidance**

Use @everything-claude-code:remotion-best-practices before editing Remotion code.

- [ ] **Step 2: Write failing schema/template tests**

Expected:

- Product categories select `ProductCardVideo`.
- Other categories select `BriefingVideo`.
- Long text is accepted but marked for fitting.

- [ ] **Step 3: Run tests**

```powershell
npm test -- tests/unit/remotion
```

Expected: FAIL.

- [ ] **Step 4: Implement compositions**

Composition IDs:

- `BriefingVideo`
- `ProductCardVideo`

Use 1080x1920, 30fps, 18-25 seconds by default.

- [ ] **Step 5: Run unit tests**

```powershell
npm test -- tests/unit/remotion
```

Expected: PASS.

- [ ] **Step 6: Run Remotion still check**

```powershell
npm run remotion:still
```

Expected: `out/still.png` exists and contains nonblank content.

- [ ] **Step 7: Commit**

```powershell
git add src/remotion tests/unit/remotion out/.gitkeep
git commit -m "feat: add remotion video templates"
```

Do not commit generated `out/still.png`.

## Task 11: Implement Render Service

**Files:**
- Create: `src/remotion/render/renderVideo.ts`
- Create: `src/remotion/render/renderSample.ts`
- Modify: `src/server/db/repositories/packs.ts`
- Test: `tests/integration/remotion/renderStill.test.ts`
- Test: `tests/integration/remotion/renderSample.test.ts`

- [ ] **Step 1: Write failing render service test**

Test should render one low-scale still and one short low-scale mp4 sample with a long headline fixture. Fake renderer wiring is allowed only for `generate-today` job tests, not for this render smoke test.

- [ ] **Step 2: Run test**

```powershell
npm test -- tests/integration/remotion/renderStill.test.ts tests/integration/remotion/renderSample.test.ts
```

Expected: FAIL.

- [ ] **Step 3: Implement render wrapper**

Use `@remotion/renderer` from server-side code. Render one item at a time and record failures per pack.
The sample render path must render frames `0-90` at `scale=0.25` and fail if output is missing or zero bytes.

- [ ] **Step 4: Run still verification**

```powershell
npm run remotion:still
```

Expected: PASS and visually inspect the still.

- [ ] **Step 5: Run short mp4 render verification**

```powershell
npm run remotion:sample
```

Expected: `out/sample.mp4` exists, is nonzero, and the long headline remains inside the frame.

- [ ] **Step 6: Commit**

```powershell
git add src/remotion/render src/server/db/repositories tests/integration/remotion
git commit -m "feat: add remotion render service"
```

## Task 12: Implement `generate-today` CLI Job

**Files:**
- Create: `src/server/jobs/generateToday.ts`
- Create: `src/server/cli.ts`
- Test: `tests/integration/jobs/generateToday.test.ts`
- Test: `tests/unit/server/cli.test.ts`

- [ ] **Step 1: Write failing job integration test**

Use fake provider, manual override fixture, and fake renderer:

```ts
const result = await generateToday({
  date: "2026-04-29",
  aiProvider: fakeProvider,
  renderer: fakeRenderer,
  collectors: [manualCollectorFromFixture("tests/fixtures/manual-overrides.json")],
});
expect(result.postPackCount).toBe(10);
expect(fs.existsSync("exports/2026-04-29/manifest.json")).toBe(true);
```

- [ ] **Step 2: Run test**

```powershell
npm test -- tests/integration/jobs/generateToday.test.ts
```

Expected: FAIL.

- [ ] **Step 3: Implement orchestration**

Order:

1. Load env and settings.
2. Migrate DB.
3. Run collectors.
4. Deduplicate.
5. Score and select 10.
6. Generate text.
7. Render video.
8. Write exports.
9. Save job result and errors.

- [ ] **Step 4: Implement CLI command**

Support:

```powershell
npm run generate:today -- --date 2026-04-29 --env-file .env
```

Also support:

```powershell
npm run serve -- --host 127.0.0.1 --port 4173 --env-file .env
```

`serve` must start the local API server and serve the built dashboard assets when they exist. In development, `npm run api` plus `npm run dev` is allowed, but `serve` is the production/local operations command documented for users.

- [ ] **Step 5: Run tests**

```powershell
npm test -- tests/integration/jobs/generateToday.test.ts tests/unit/server/cli.test.ts
```

Expected: PASS.

- [ ] **Step 6: Run manual fake generation**

```powershell
$env:AI_PROVIDER='fake'
npm run generate:today -- --date 2026-04-29
```

Expected: SQLite DB created and `exports/2026-04-29/manifest.json` exists.

- [ ] **Step 7: Commit**

```powershell
git add src/server/jobs src/server/cli.ts tests/integration/jobs tests/unit/server
git commit -m "feat: add daily generation cli"
```

## Task 13: Implement API Server

**Files:**
- Create: `src/server/api/server.ts`
- Create: `src/server/api/routes/today.ts`
- Create: `src/server/api/routes/packs.ts`
- Create: `src/server/api/routes/settings.ts`
- Create: `src/server/api/routes/regenerate.ts`
- Create: `src/server/api/routes/generate.ts`
- Create: `src/server/api/routes/candidates.ts`
- Test: `tests/integration/api/today.test.ts`
- Test: `tests/integration/api/settings.test.ts`
- Test: `tests/integration/api/regenerate.test.ts`
- Test: `tests/integration/api/generate.test.ts`
- Test: `tests/integration/api/candidates.test.ts`

- [ ] **Step 1: Write failing API tests**

Expected:

- `GET /api/today?date=2026-04-29` returns 10 packs.
- `POST /api/packs/:id/posting-status` marks a pack posted.
- `POST /api/regenerate/comment` regenerates comment only through fake provider.
- `POST /api/regenerate/video` queues or runs video regeneration for one pack.
- `POST /api/candidates/:packId/replace` replaces one selected candidate and regenerates related text/render state.
- `POST /api/generate/today` triggers the same generation path as CLI for manual dashboard execution.

- [ ] **Step 2: Run tests**

```powershell
npm test -- tests/integration/api
```

Expected: FAIL.

- [ ] **Step 3: Implement Express app factory**

Do not bind a port during tests. Export `createServer()`.
All manual execution endpoints must be local-only and must reject concurrent generation jobs with a clear 409 response.

- [ ] **Step 4: Run tests**

```powershell
npm test -- tests/integration/api
```

Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add src/server/api tests/integration/api
git commit -m "feat: add local dashboard api"
```

## Task 14: Implement Dashboard UI

**Files:**
- Create: `src/dashboard/main.tsx`
- Create: `src/dashboard/App.tsx`
- Create: `src/dashboard/api/client.ts`
- Create: `src/dashboard/pages/TodayPage.tsx`
- Create: `src/dashboard/pages/DetailPage.tsx`
- Create: `src/dashboard/pages/SettingsPage.tsx`
- Create: `src/dashboard/pages/AnalyticsPage.tsx`
- Create: `src/dashboard/components/*.tsx`
- Create: `src/dashboard/styles.css`
- Test: `tests/unit/dashboard/TodayPage.test.tsx`
- Test: `tests/unit/dashboard/DetailPage.test.tsx`

- [ ] **Step 1: Write failing component tests**

Cover:

- Today page renders 10 packs.
- Low-trust item displays review reason.
- SNS copy buttons render for all four SNS targets.
- Posted checkbox calls API client.
- Manual "generate today" button calls `POST /api/generate/today` and shows running/success/error states.
- Detail page exposes comment regeneration, video regeneration, and candidate replacement actions.
- Candidate replacement UI shows available alternatives and requires confirmation before replacing.

- [ ] **Step 2: Run tests**

```powershell
npm test -- tests/unit/dashboard
```

Expected: FAIL.

- [ ] **Step 3: Implement UI**

Design rules:

- Quiet Mac-like app UI.
- No landing page.
- Dense but readable operational dashboard.
- Icons from `lucide-react`.
- No nested cards.
- Buttons use icons where obvious.
- Long labels must wrap without resizing fixed toolbars or action rows.

- [ ] **Step 4: Run component tests**

```powershell
npm test -- tests/unit/dashboard
```

Expected: PASS.

- [ ] **Step 5: Run Vite build**

```powershell
npm run build
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add src/dashboard tests/unit/dashboard
git commit -m "feat: add posting dashboard"
```

## Task 15: Add Browser and X.com Readonly Collectors

**Files:**
- Create: `src/server/collectors/browser.ts`
- Create: `src/server/collectors/xReadonly.ts`
- Modify: `src/server/collectors/policy.ts`
- Test: `tests/unit/collectors/browserPolicy.test.ts`
- Test: `tests/unit/collectors/xReadonly.test.ts`

- [ ] **Step 1: Use security review skill**

Use @everything-claude-code:security-review before adding browser/X.com code.

- [ ] **Step 2: Write failing policy tests**

Expected:

- Disabled unless `ENABLE_BROWSER_COLLECTORS=true`.
- X.com disabled unless `ENABLE_X_READONLY=true`.
- CAPTCHA/login wall/account warning returns skipped result.
- Collector never stores username, password, cookie, or session values.

- [ ] **Step 3: Run tests**

```powershell
npm test -- tests/unit/collectors/browserPolicy.test.ts tests/unit/collectors/xReadonly.test.ts
```

Expected: FAIL.

- [ ] **Step 4: Implement safe wrappers**

Implementation rules:

- Read-only navigation and extraction only.
- No form submit, click-post, like, follow, DM, notification action.
- Save trend label, display text, URL, collectedAt, collector ID.
- Stop and fallback on login wall, CAPTCHA, account warning, explicit forbidden page, or rate-limit state.

- [ ] **Step 5: Run tests**

```powershell
npm test -- tests/unit/collectors/browserPolicy.test.ts tests/unit/collectors/xReadonly.test.ts
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add src/server/collectors tests/unit/collectors
git commit -m "feat: add safe browser collectors"
```

## Task 16: Add Windows Task Scheduler Scripts

**Files:**
- Create: `scripts/run-generate.ps1`
- Create: `scripts/install-schedule.ps1`
- Test: `tests/unit/scripts/scheduleCommand.test.ts`

- [ ] **Step 1: Write failing command-generation test**

Expected generated task:

- Name: `SocialVideoStudioDailyGenerate`
- Time: 06:00
- Working directory: `D:\NEXTCLOUD\Windows_app\SocialVideoStudio`
- Command: `powershell -ExecutionPolicy Bypass -File scripts\run-generate.ps1`
- Includes `--env-file .env` by default, or a custom env file when passed.
- Records/displays the Windows execution user used for task registration.
- Logs to `logs/scheduler-YYYY-MM-DD.log`.
- Creates the log directory before writing.
- Writes `data/schedule-manifest.json` with `TaskName`, `Time`, resolved `EnvFile`, `RunAsUser`, working directory, action command, and log path.
- Reads the manifest back in the unit test to verify persisted values match the registered task action.

- [ ] **Step 2: Run tests**

```powershell
npm test -- tests/unit/scripts/scheduleCommand.test.ts
```

Expected: FAIL.

- [ ] **Step 3: Implement scripts**

`run-generate.ps1` should accept an `-EnvFile` parameter with default `.env`:

```powershell
param(
  [string]$EnvFile = ".env"
)
npm run generate:today -- --env-file $EnvFile
```

and append stdout/stderr to `logs`. `install-schedule.ps1` should accept `-TaskName`, `-Time`, `-EnvFile`, and `-RunAsUser` parameters, echo the resolved values, write the same values to a local schedule manifest for dashboard display, and register the task action as:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\run-generate.ps1 -EnvFile "<resolved-env-file>"
```

- [ ] **Step 4: Run tests**

```powershell
npm test -- tests/unit/scripts/scheduleCommand.test.ts
```

Expected: PASS.

- [ ] **Step 5: Manual schedule dry run**

Run:

```powershell
npm run schedule:run
```

Expected: same behavior as `generate:today`.

- [ ] **Step 6: Commit**

```powershell
git add scripts tests/unit/scripts
git commit -m "feat: add windows scheduler scripts"
```

## Task 17: Add E2E Dashboard Verification

**Files:**
- Create: `playwright.config.ts`
- Create: `tests/e2e/dashboard.spec.ts`
- Modify: `package.json`

- [ ] **Step 1: Write E2E test**

Flow:

1. Seed fake generated data.
2. Start API and Vite app.
3. Open Today page.
4. Verify 10 items.
5. Open first detail.
6. Copy X text.
7. Mark posted.
8. Trigger comment regeneration.
9. Trigger video regeneration and verify render status changes.
10. Replace a candidate and verify the detail page reflects the new source/title.
11. Click the manual generate button and verify the UI shows the generation job state.

- [ ] **Step 2: Run E2E test**

```powershell
npm run test:e2e
```

Expected: FAIL until app wiring is complete.

- [ ] **Step 3: Fix app wiring only**

Do not broaden scope. Make the tested flow pass.

- [ ] **Step 4: Re-run E2E**

```powershell
npm run test:e2e
```

Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add playwright.config.ts tests/e2e package.json
git commit -m "test: add dashboard e2e flow"
```

## Task 18: Add Documentation and Final Verification

**Files:**
- Modify: `README.md`
- Create: `docs/operations.md`
- Create: `docs/safety.md`
- Create: `docs/sources.md`

- [ ] **Step 1: Document setup**

README must include:

- Node/npm setup.
- `.env` creation.
- Fake-provider smoke test.
- OpenAI provider config.
- Local LLM config.
- Daily generation command.
- Dashboard command.
- Scheduler install command.

- [ ] **Step 2: Document safety**

`docs/safety.md` must include:

- X.com readonly policy.
- External source policy.
- Image license policy.
- Secret redaction policy.
- No automatic posting.

- [ ] **Step 3: Run full verification**

Run:

```powershell
npm run typecheck
npm test
npm run build
npm run remotion:still
npm run remotion:sample
npm run schedule:run
```

Expected: all pass. `schedule:run` creates today's export using fake provider unless configured otherwise.

- [ ] **Step 4: Run E2E**

```powershell
npm run test:e2e
```

Expected: PASS.

- [ ] **Step 5: Inspect generated export**

Check:

```powershell
Get-ChildItem -Recurse 'exports'
```

Expected: date folder with manifest, CSV, SNS text files, and rendered video outputs or fake-render outputs depending on test mode.

- [ ] **Step 6: Commit**

```powershell
git add README.md docs
git commit -m "docs: add social video studio operations guide"
```

## Task 19: Final Release Commit and Push

**Files:**
- No new files expected unless verification fixes are needed.

- [ ] **Step 1: Check status**

```powershell
git status --short --branch
```

Expected: clean working tree in `D:/NEXTCLOUD/Windows_app/SocialVideoStudio`.

- [ ] **Step 2: Review commit history**

```powershell
git log --oneline --max-count=20
```

Expected: task-sized commits.

- [ ] **Step 3: Push**

If the new repo has no remote, ask the user for the remote URL before pushing.

If a remote exists:

```powershell
git push -u origin main
```

Expected: push succeeds.

- [ ] **Step 4: Return to ZWG repo and commit plan if not already committed**

```powershell
Set-Location 'D:\NEXTCLOUD\Windows_app\ZWG_Terminal'
git status --short --branch
```

Expected: no staged implementation files from SocialVideoStudio because app is separate.

## Verification Gate

Do not claim implementation complete until these pass in `D:/NEXTCLOUD/Windows_app/SocialVideoStudio`:

```powershell
npm run typecheck
npm test
npm run build
npm run remotion:still
npm run remotion:sample
npm run schedule:run
npm run test:e2e
```

Also verify:

- The project root is not inside `D:/NEXTCLOUD/Windows_app/ZWG_Terminal`.
- `.env` is not committed.
- Generated `exports/`, logs, SQLite DB, and rendered media are not committed.
- X.com collector is disabled by default.
- Browser collectors are disabled by default.
- Fake provider can create a full 10-item export without secrets.
- `serve` starts the local operational app without relying on the Vite dev server.
- Short mp4 sample render passes with a long headline fixture.
