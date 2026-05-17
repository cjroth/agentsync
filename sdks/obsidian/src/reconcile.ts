// Startup convergence between the Obsidian-side vault content and the
// SDK-side document. Pure planning function — takes minimal interfaces,
// no Obsidian or SDK runtime dependencies, fully unit-testable.
//
// Automerge handles concurrent-write convergence in steady state; this
// module's only job is to bring the two sides into byte-equality before
// live event-driven sync starts streaming changes. Once reconcile is done
// the bridge takes over and `vault.subscribe('doc-changed')` + Obsidian
// vault events handle everything thereafter.

export interface ObsidianFileSummary {
  path: string;
  readText(): Promise<string>;
}

export interface SdkFileSummary {
  path: string;
  /** True when the SDK has a tombstone for this path. */
  deleted: boolean;
  /** Only called when `deleted` is false. */
  readText(): Promise<string>;
}

export interface ReconcileInputs {
  obsidianFiles: readonly ObsidianFileSummary[];
  sdkFiles: readonly SdkFileSummary[];
  /** Path filter — paths that fail this are excluded from both sides. */
  filter: (path: string) => boolean;
}

export interface PlannedSdkWrite {
  path: string;
  content: string;
}

export interface PlannedObsidianWrite {
  path: string;
  content: string;
  /** True when the file does not yet exist locally and must be created. */
  create: boolean;
}

export interface ReconcilePlan {
  pushToSdk: PlannedSdkWrite[];
  applyToObsidian: PlannedObsidianWrite[];
  deleteInObsidian: string[];
}

/**
 * Compute the work needed to make Obsidian and the SDK byte-equal, given
 * the present state of both sides. Algorithm, per path in the union:
 *
 *  - both alive, equal bytes  → no-op
 *  - obsidian-only            → push to SDK
 *  - both alive, differ       → push obsidian's content to SDK (Automerge
 *                                merges it with whatever the remote has)
 *  - sdk-only, alive          → write to obsidian
 *  - sdk tombstone + obsidian → delete in obsidian
 */
export async function planReconcile(inputs: ReconcileInputs): Promise<ReconcilePlan> {
  const plan: ReconcilePlan = {
    pushToSdk: [],
    applyToObsidian: [],
    deleteInObsidian: [],
  };

  const obs = new Map<string, ObsidianFileSummary>();
  for (const f of inputs.obsidianFiles) {
    if (inputs.filter(f.path)) obs.set(f.path, f);
  }

  const sdk = new Map<string, SdkFileSummary>();
  for (const f of inputs.sdkFiles) {
    if (inputs.filter(f.path)) sdk.set(f.path, f);
  }

  const allPaths = new Set<string>();
  for (const k of obs.keys()) allPaths.add(k);
  for (const k of sdk.keys()) allPaths.add(k);

  for (const path of allPaths) {
    const o = obs.get(path);
    const s = sdk.get(path);

    if (o && s && s.deleted) {
      plan.deleteInObsidian.push(path);
      continue;
    }

    if (o && s && !s.deleted) {
      const oContent = await o.readText();
      const sContent = await s.readText();
      if (oContent !== sContent) {
        plan.pushToSdk.push({ path, content: oContent });
      }
      continue;
    }

    if (o && !s) {
      const content = await o.readText();
      plan.pushToSdk.push({ path, content });
      continue;
    }

    if (!o && s && !s.deleted) {
      const content = await s.readText();
      plan.applyToObsidian.push({ path, content, create: true });
    }
    // Else: !o && (no s OR s.deleted) — nothing to do.
  }

  return plan;
}
