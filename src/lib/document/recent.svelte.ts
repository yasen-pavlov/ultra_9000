// Recent Markdown per project for the palette's Documents section (board 33). The last list per
// project shows at once; each palette open asks native again (native scans off the main thread
// and caches too).
import { errorMessage, projectDocuments, type DocumentList } from "../native";

class RecentDocuments {
  lists = $state<Record<string, DocumentList>>({});
  /** The last scan failure per project, cleared by the next success. */
  errors = $state<Record<string, string>>({});

  refresh(projectIds: readonly string[]): void {
    for (const projectId of projectIds) {
      projectDocuments(projectId).then(
        (list) => {
          this.lists[projectId] = list;
          delete this.errors[projectId];
        },
        (error) => {
          this.errors[projectId] = errorMessage(error);
        },
      );
    }
  }
}

export const recentDocuments = new RecentDocuments();
