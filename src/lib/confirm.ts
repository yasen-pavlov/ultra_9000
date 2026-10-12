// The close question's wording (board 13), shared by the app's close sheet and the menu-bar
// popover's inline question so both ask exactly the same thing.
import { basename } from "./format";
import type { CloseReason, ItemState } from "./native";

export interface CopyPart {
  text: string;
  /** Emphasised: the consequence the user must not miss. */
  strong: boolean;
}

export interface CopyParagraph {
  parts: CopyPart[];
  /** Secondary reassurance (what is kept). */
  dim: boolean;
}

export interface CloseQuestion {
  /** `End “Fix the parser”?` */
  title: string;
  /** "End agent" / "End shell" / "End Neovim". */
  endLabel: string;
  body: CopyParagraph[];
}

export interface CloseQuestionInput {
  item: ItemState | null;
  reason: CloseReason;
  /** The shell's foreground job, when known. */
  detail: string | null;
  /** Tool label ("OMP"); "" when the item is gone. */
  toolLabel: string;
  /** Configured executable path of the item's tool, if any. */
  toolPath: string | null;
  /** Project display name. */
  project: string;
}

const plain = (text: string): CopyPart => ({ text, strong: false });
const strong = (text: string): CopyPart => ({ text, strong: true });

/** What a waiting agent asks for; an agent without an attention kind waits for a reply. */
const WAITING_FOR: Record<NonNullable<ItemState["attention"]>, string> = {
  question: "answer to a question",
  approval: "approval",
};

/** The End button; every other reason ends an agent. */
const END_LABEL: Partial<Record<CloseReason, string>> = {
  "foreground-job": "End shell",
  neovim: "End Neovim",
};

export function closeQuestion({
  item,
  reason,
  detail,
  toolLabel,
  toolPath,
  project,
}: CloseQuestionInput): CloseQuestion {
  const saved: CopyParagraph = {
    parts: [plain(`The conversation up to the last completed turn stays saved in ${project}, so you can resume it.`)],
    dim: true,
  };
  let body: CopyParagraph[];
  switch (reason) {
    case "agent-working":
      body = [
        {
          parts: [
            plain(`${toolLabel} is in the middle of a turn. Ending now stops it: `),
            strong("this turn's unfinished work is lost"),
            plain(". Changes it already wrote to files stay written."),
          ],
          dim: false,
        },
        saved,
      ];
      break;
    case "agent-needs-you": {
      const attention = item?.attention ?? null;
      const waitingFor = attention === null ? "reply" : WAITING_FOR[attention];
      body = [
        {
          parts: [
            plain(`${toolLabel} is waiting for your ${waitingFor}. Ending now drops that pending turn: `),
            strong("its unfinished work is lost"),
            plain(". Changes it already wrote to files stay written."),
          ],
          dim: false,
        },
        saved,
      ];
      break;
    }
    case "foreground-job": {
      // The executable label is stable even when zsh sets the terminal title to the job command.
      const shell = item === null ? "The shell" : basename(toolPath ?? "") || toolLabel;
      body = [
        {
          parts: [
            plain(`${shell} is running `),
            strong(detail ?? "a foreground job"),
            plain(". Ending the shell ends that job too, and the screen and scrollback are gone. "),
            strong("This can't be undone."),
          ],
          dim: false,
        },
      ];
      break;
    }
    case "neovim":
      body = [
        {
          parts: [
            plain("Neovim may hold unsaved buffers that Ultra 9000 cannot see. Ending it sends a hangup: "),
            strong("unsaved changes may be lost"),
            plain("."),
          ],
          dim: false,
        },
        { parts: [plain("Neovim's own swap files may help recover them.")], dim: true },
      ];
      break;
    default:
      body = [
        {
          parts: [
            plain(
              `${toolLabel} reports no turn state, so Ultra 9000 can't tell whether it is in the middle of something. Ending it stops the process.`,
            ),
          ],
          dim: false,
        },
        { parts: [plain(`${toolLabel} keeps its own saved conversation; resume it from the palette.`)], dim: true },
      ];
      break;
  }
  return {
    title: `End “${item?.title ?? "this item"}”?`,
    endLabel: END_LABEL[reason] ?? "End agent",
    body,
  };
}
