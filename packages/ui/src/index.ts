/// <reference path="./css.d.ts" />

export { App, type AppProps } from './App'

export { TitleBar, type TitleBarProps } from './shell/TitleBar'
export { MenuBar, type MenuBarProps } from './shell/MenuBar'
export { Ribbon, type RibbonProps } from './shell/Ribbon'
export { StatusBar, type StatusBarProps } from './shell/StatusBar'
export { ShortcutsDialog, type ShortcutsDialogProps } from './shell/ShortcutsDialog'

export { ViewPanel, type ViewPanelProps } from './view/ViewPanel'
export { ChannelBar, type ChannelBarProps } from './view/ChannelBar'
export {
  ChannelSettingsPopover,
  type ChannelSettingsPopoverProps,
} from './view/ChannelSettingsPopover'
export { Stage, type StageProps } from './view/Stage'
export { Transport } from './view/Transport'
export { usePlayback, PLAYBACK_INTERVAL_MS } from './view/usePlayback'

export { Inspector, type InspectorProps, type InspectorTab } from './inspector/Inspector'
export { InspectPanel, type InspectPanelProps } from './inspector/InspectPanel'
export { LineagePanel, type LineagePanelProps } from './inspector/LineagePanel'
export { HistoryPanel, type HistoryPanelProps } from './inspector/HistoryPanel'

export * from './lib/keymap'
export * from './lib/channels'
export * from './lib/histogram'
export * from './lib/transport'
export * from './lib/hud'
export * from './lib/advisories'
export * from './lib/axisScale'
export * from './lib/lineage'
export * from './lib/format'

export type {
  BackendState,
  CursorSample,
  DisplayState,
  HistoryEntry,
  MenuId,
  PerfSample,
  ProjectStatus,
} from './types'
