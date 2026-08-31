import { lazyNamed } from "@/core/lazyNamed";

export const DiagnosticsView = lazyNamed(
  () => import("@/views/DiagnosticsView"),
  (module) => module.DiagnosticsView,
);
export const LibraryView = lazyNamed(
  () => import("@/views/LibraryView"),
  (module) => module.LibraryView,
);
export const SettingsView = lazyNamed(
  () => import("@/views/SettingsView"),
  (module) => module.SettingsView,
);

export const PlaylistsView = lazyNamed(
  () => import("@/views/PlaylistsView"),
  (module) => module.PlaylistsView,
);
export const SoundboardsView = lazyNamed(
  () => import("@/views/SoundboardsView"),
  (module) => module.SoundboardsView,
);
export const InterruptsView = lazyNamed(
  () => import("@/views/InterruptsView"),
  (module) => module.InterruptsView,
);
export const PresetsView = lazyNamed(
  () => import("@/views/PresetsView"),
  (module) => module.PresetsView,
);
export const CuesView = lazyNamed(
  () => import("@/views/CuesView"),
  (module) => module.CuesView,
);

export const PlaylistBuilderView = lazyNamed(
  () => import("@/views/assistant/PlaylistBuilderView"),
  (module) => module.PlaylistBuilderView,
);
export const EqAssistantView = lazyNamed(
  () => import("@/views/assistant/EqAssistantView"),
  (module) => module.EqAssistantView,
);
export const LibraryAnalysisView = lazyNamed(
  () => import("@/views/assistant/LibraryAnalysisView"),
  (module) => module.LibraryAnalysisView,
);
export const LibraryContextView = lazyNamed(
  () => import("@/views/assistant/LibraryContextView"),
  (module) => module.LibraryContextView,
);
export const LibraryTagsView = lazyNamed(
  () => import("@/views/assistant/LibraryTagsView"),
  (module) => module.LibraryTagsView,
);
export const AssistantAiSetupView = lazyNamed(
  () => import("@/views/assistant/AssistantAiSetupView"),
  (module) => module.AssistantAiSetupView,
);
export const TagVocabularyView = lazyNamed(
  () => import("@/views/assistant/TagVocabularyView"),
  (module) => module.TagVocabularyView,
);
const loadLibraryCleanupViews = () => import("@/views/assistant/LibraryCleanupViews");
export const LibraryCleanupRunView = lazyNamed(
  loadLibraryCleanupViews,
  (module) => module.LibraryCleanupRunView,
);
export const LibraryCleanupHistoryView = lazyNamed(
  loadLibraryCleanupViews,
  (module) => module.LibraryCleanupHistoryView,
);
export const LibraryCleanupSourcesView = lazyNamed(
  loadLibraryCleanupViews,
  (module) => module.LibraryCleanupSourcesView,
);
export const LibraryCleanupModelView = lazyNamed(
  loadLibraryCleanupViews,
  (module) => module.LibraryCleanupModelView,
);

const loadAssistantShell = () => import("./AssistantShell");

export const AssistantShell = lazyNamed(
  loadAssistantShell,
  (module) => module.AssistantShell,
);
export const AssistantSettingsShell = lazyNamed(
  loadAssistantShell,
  (module) => module.AssistantSettingsShell,
);
export const MoodLibraryShell = lazyNamed(
  loadAssistantShell,
  (module) => module.MoodLibraryShell,
);
export const LibraryCleanupShell = lazyNamed(
  loadAssistantShell,
  (module) => module.LibraryCleanupShell,
);
export const AuthoringShell = lazyNamed(
  () => import("./AuthoringShell"),
  (module) => module.AuthoringShell,
);
