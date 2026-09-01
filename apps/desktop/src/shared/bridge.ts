export interface BackendInfo {
  baseUrl: string
  token: string
  generation: number
}

export type BackendState = 'starting' | 'ready' | 'down' | 'fatal'

export interface BackendStateEvent {
  state: BackendState
  generation: number
}

export interface CellStudioBridge {
  openOnStart(): Promise<string | null>
  getBackendInfo(): Promise<BackendInfo | null>
  openDatasetDialog(): Promise<string | null>
  openTrackingDialog(): Promise<string | null>
  onBackendState(cb: (event: BackendStateEvent) => void): () => void
}

export const IPC = {
  backendInfo: 'backend:info',
  openDataset: 'dialog:open-dataset',
  openTracking: 'dialog:open-tracking',
  openOnStart: 'app:open-on-start',
  backendState: 'backend:state',
} as const
