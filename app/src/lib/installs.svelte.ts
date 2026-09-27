/**
 * Installs in flight, wherever they were started from.
 *
 * A download can begin from the browser window, from the Library tab, or from
 * a click on nexusmods.com in a completely different application, and it has
 * to keep reporting after the user navigates away from whichever tab started
 * it. So the queue lives here, outside any component, rather than in the one
 * that happened to be open at the time.
 */

import { humanBytes, installArchive, installFromNxm, type InstallStage } from "./engine";
import { unmergedNote } from "./types";

/**
 * The install's own notes, plus a word about anything it unmade.
 *
 * Installing over a mod that a combined build was made from takes that build
 * away -- it holds the edits this install just replaced. The queue already
 * shows notes, so this is where it belongs rather than in a dialog nobody
 * asked for.
 */
function noticesFor(done: { notes: string[]; dissolved: string[] } | null): string[] {
  if (!done) return [];
  const unmerged = unmergedNote(done.dissolved, "the version you just replaced");
  return unmerged ? [...done.notes, unmerged] : done.notes;
}

export interface Job {
  id: number;
  /** what we can name it before the archive is open; the mod name after */
  label: string;
  stage: InstallStage;
  /** set once it has finished, one way or the other */
  outcome: "installed" | "failed" | null;
  detail: string | null;
  notes: string[];
}

let nextId = 1;

class Queue {
  jobs = $state<Job[]>([]);

  /** Jobs still running, worst-case the thing a footer should count. */
  get running(): Job[] {
    return this.jobs.filter((j) => j.outcome === null);
  }

  get latest(): Job | null {
    return this.jobs.length ? this.jobs[this.jobs.length - 1] : null;
  }

  private add(label: string): Job {
    const job: Job = {
      id: nextId++,
      label,
      stage: { step: "resolving" },
      outcome: null,
      detail: null,
      notes: [],
    };
    this.jobs = [...this.jobs, job];
    return job;
  }

  private patch(id: number, changes: Partial<Job>) {
    this.jobs = this.jobs.map((j) => (j.id === id ? { ...j, ...changes } : j));
  }

  /** Progress events carry no job id, so they land on whatever is running. */
  report(stage: InstallStage) {
    const live = this.running;
    if (!live.length) return;
    this.patch(live[live.length - 1].id, { stage });
  }

  clear() {
    this.jobs = this.jobs.filter((j) => j.outcome === null);
  }

  async fromLink(url: string, modsDir?: string, gameRoot?: string) {
    const job = this.add("Download from Nexus");
    try {
      const done = await installFromNxm(url, modsDir, gameRoot);
      this.patch(job.id, {
        label: done?.owner ?? job.label,
        stage: { step: "done" },
        outcome: "installed",
        detail: done
          ? `${done.files} ${done.files === 1 ? "file" : "files"} installed`
          : null,
        notes: noticesFor(done),
      });
      return done;
    } catch (err) {
      this.patch(job.id, { outcome: "failed", detail: String(err) });
      throw err;
    }
  }

  async fromFile(
    archive: string,
    modsDir?: string,
    gameRoot?: string,
    overwrite = false,
  ) {
    const name = archive.split(/[\\/]/).pop() ?? archive;
    const job = this.add(name);
    try {
      const done = await installArchive(archive, modsDir, gameRoot, overwrite);
      this.patch(job.id, {
        label: done?.owner ?? name,
        stage: { step: "done" },
        outcome: "installed",
        detail: done
          ? `${done.files} ${done.files === 1 ? "file" : "files"} installed`
          : null,
        notes: noticesFor(done),
      });
      return done;
    } catch (err) {
      this.patch(job.id, { outcome: "failed", detail: String(err) });
      throw err;
    }
  }
}

export const installs = new Queue();

/** What a job is doing, said plainly. */
export function describe(job: Job): string {
  if (job.outcome === "failed") return job.detail ?? "Failed";
  if (job.outcome === "installed") return job.detail ?? "Installed";
  switch (job.stage.step) {
    case "resolving":
      return "Asking Nexus where to get it…";
    case "downloading": {
      const { bytes, total } = job.stage;
      return total
        ? `Downloading ${humanBytes(bytes)} of ${humanBytes(total)}`
        : `Downloading ${humanBytes(bytes)}`;
    }
    case "extracting":
      return "Unpacking it…";
    case "deploying":
      return "Putting it in the game…";
    case "done":
      return "Installed";
  }
}

/** 0–1, or null when the size is not known yet. */
export function fraction(job: Job): number | null {
  if (job.stage.step !== "downloading") return null;
  const { bytes, total } = job.stage;
  return total ? Math.min(bytes / total, 1) : null;
}
