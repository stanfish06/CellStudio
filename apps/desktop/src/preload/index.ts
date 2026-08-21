import { contextBridge, ipcRenderer } from 'electron'
import { IPC, type BackendStateEvent, type CellStudioBridge } from '../shared/bridge'

const bridge: CellStudioBridge = {
  getBackendInfo: () => ipcRenderer.invoke(IPC.backendInfo),
  openOnStart: () => ipcRenderer.invoke(IPC.openOnStart),
  openDatasetDialog: () => ipcRenderer.invoke(IPC.openDataset),
  onBackendState: (cb) => {
    const handler = (_e: unknown, event: BackendStateEvent) => cb(event)
    ipcRenderer.on(IPC.backendState, handler)
    return () => ipcRenderer.removeListener(IPC.backendState, handler)
  },
}

contextBridge.exposeInMainWorld('cellstudio', bridge)
