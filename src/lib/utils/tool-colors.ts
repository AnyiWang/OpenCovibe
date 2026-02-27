export interface ToolColor {
  bg: string;
  text: string;
  icon: string;
}

export const toolColors: Record<string, ToolColor> = {
  read_file: {
    bg: "bg-blue-500/10",
    text: "text-blue-500 dark:text-blue-400",
    icon: "M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2zM22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z",
  },
  Read: {
    bg: "bg-blue-500/10",
    text: "text-blue-500 dark:text-blue-400",
    icon: "M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2zM22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z",
  },
  write_file: {
    bg: "bg-amber-500/10",
    text: "text-amber-600 dark:text-amber-400",
    icon: "M12 20h9M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z",
  },
  Write: {
    bg: "bg-amber-500/10",
    text: "text-amber-600 dark:text-amber-400",
    icon: "M12 20h9M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z",
  },
  edit_file: {
    bg: "bg-amber-500/10",
    text: "text-amber-600 dark:text-amber-400",
    icon: "M12 20h9M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z",
  },
  Edit: {
    bg: "bg-amber-500/10",
    text: "text-amber-600 dark:text-amber-400",
    icon: "M12 20h9M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z",
  },
  bash: {
    bg: "bg-emerald-500/10",
    text: "text-emerald-600 dark:text-emerald-400",
    icon: "M4 17l6-6-6-6M12 19h8",
  },
  Bash: {
    bg: "bg-emerald-500/10",
    text: "text-emerald-600 dark:text-emerald-400",
    icon: "M4 17l6-6-6-6M12 19h8",
  },
  list_directory: {
    bg: "bg-purple-500/10",
    text: "text-purple-600 dark:text-purple-400",
    icon: "M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z",
  },
  search_files: {
    bg: "bg-purple-500/10",
    text: "text-purple-600 dark:text-purple-400",
    icon: "M11 3a8 8 0 1 0 0 16 8 8 0 0 0 0-16zM21 21l-4.35-4.35",
  },
  Grep: {
    bg: "bg-purple-500/10",
    text: "text-purple-600 dark:text-purple-400",
    icon: "M11 3a8 8 0 1 0 0 16 8 8 0 0 0 0-16zM21 21l-4.35-4.35",
  },
  Glob: {
    bg: "bg-purple-500/10",
    text: "text-purple-600 dark:text-purple-400",
    icon: "M11 3a8 8 0 1 0 0 16 8 8 0 0 0 0-16zM21 21l-4.35-4.35",
  },
  Task: {
    bg: "bg-cyan-500/10",
    text: "text-cyan-600 dark:text-cyan-400",
    icon: "M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2",
  },
  AskUserQuestion: {
    bg: "bg-yellow-500/10",
    text: "text-yellow-600 dark:text-yellow-400",
    icon: "M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3M12 17h.01M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20z",
  },
  WebFetch: {
    bg: "bg-sky-500/10",
    text: "text-sky-600 dark:text-sky-400",
    icon: "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20zM2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z",
  },
  WebSearch: {
    bg: "bg-sky-500/10",
    text: "text-sky-600 dark:text-sky-400",
    icon: "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20zM2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z",
  },
  TaskOutput: {
    bg: "bg-cyan-500/10",
    text: "text-cyan-600 dark:text-cyan-400",
    icon: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3",
  },
  TaskStop: { bg: "bg-red-500/10", text: "text-red-600 dark:text-red-400", icon: "M6 6h12v12H6z" },
  TaskCreate: {
    bg: "bg-teal-500/10",
    text: "text-teal-600 dark:text-teal-400",
    icon: "M12 5v14M5 12h14",
  },
  TaskGet: {
    bg: "bg-cyan-500/10",
    text: "text-cyan-600 dark:text-cyan-400",
    icon: "M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8zM12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z",
  },
  TaskUpdate: {
    bg: "bg-cyan-500/10",
    text: "text-cyan-600 dark:text-cyan-400",
    icon: "M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15",
  },
  TaskList: {
    bg: "bg-cyan-500/10",
    text: "text-cyan-600 dark:text-cyan-400",
    icon: "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
  },
  NotebookEdit: {
    bg: "bg-violet-500/10",
    text: "text-violet-600 dark:text-violet-400",
    icon: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M8 13l2 2 4-4",
  },
  EnterPlanMode: {
    bg: "bg-indigo-500/10",
    text: "text-indigo-600 dark:text-indigo-400",
    icon: "M12 2l4 4-4 4M12 22l-4-4 4-4M20 12H4",
  },
  ExitPlanMode: {
    bg: "bg-indigo-500/10",
    text: "text-indigo-600 dark:text-indigo-400",
    icon: "M9 11l3 3L22 4M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11",
  },
  Skill: {
    bg: "bg-rose-500/10",
    text: "text-rose-600 dark:text-rose-400",
    icon: "M13 2L3 14h9l-1 8 10-12h-9l1-8z",
  },
  TeamCreate: {
    bg: "bg-teal-500/10",
    text: "text-teal-600 dark:text-teal-400",
    icon: "M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2M9 7a4 4 0 1 0 0 8 4 4 0 0 0 0-8zM23 21v-2a4 4 0 0 1-3-3.87M16 3.13a4 4 0 0 1 0 7.75",
  },
  TeamDelete: {
    bg: "bg-red-500/10",
    text: "text-red-600 dark:text-red-400",
    icon: "M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2M9 7a4 4 0 1 0 0 8 4 4 0 0 0 0-8zM23 6l-6 6M17 6l6 6",
  },
  SendMessage: {
    bg: "bg-violet-500/10",
    text: "text-violet-600 dark:text-violet-400",
    icon: "M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z",
  },
};

export const defaultToolColor: ToolColor = {
  bg: "bg-muted",
  text: "text-muted-foreground",
  icon: "M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5",
};

export function getToolColor(name: string): ToolColor {
  return toolColors[name] ?? defaultToolColor;
}
