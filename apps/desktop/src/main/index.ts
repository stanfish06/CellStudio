import { join } from 'node:path'
import { app, BrowserWindow, dialog, ipcMain } from 'electron'
import { IPC, type BackendState } from '../shared/bridge'
import { BackendSupervisor } from './backend'

let supervisor: BackendSupervisor | null = null

function broadcast(state: BackendState, generation: number): void {
  for (const win of BrowserWindow.getAllWindows()) {
    win.webContents.send(IPC.backendState, { state, generation })
  }
}

function createWindow(): BrowserWindow {
  const win = new BrowserWindow({
    width: 1600,
    height: 1000,
    minWidth: 1100,
    minHeight: 700,
    show: false,
    backgroundColor: '#1b1c1e',
    title: 'CellStudio',
    webPreferences: {
      preload: join(__dirname, '../preload/index.mjs'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
  })

  win.once('ready-to-show', () => win.show())

  if (process.env.ELECTRON_RENDERER_URL) {
    void win.loadURL(process.env.ELECTRON_RENDERER_URL)
  } else {
    void win.loadFile(join(__dirname, '../renderer/index.html'))
  }
  return win
}

ipcMain.handle(IPC.backendInfo, () => supervisor?.info ?? null)

ipcMain.handle(IPC.openOnStart, () => process.env.CELLSTUDIO_OPEN_ON_START ?? null)

ipcMain.handle(IPC.openDataset, async () => {
  const result = await dialog.showOpenDialog({
    title: 'Open OME-Zarr dataset',
    properties: ['openDirectory'],
  })
  return result.canceled ? null : (result.filePaths[0] ?? null)
})

ipcMain.handle(IPC.openTracking, async () => {
  const result = await dialog.showOpenDialog({
    title: 'Import tracking data',
    properties: ['openFile'],
    filters: [
      { name: 'CellStudio tracking', extensions: ['json', 'gz'] },
      { name: 'All files', extensions: ['*'] },
    ],
  })
  return result.canceled ? null : (result.filePaths[0] ?? null)
})

void app.whenReady().then(async () => {
  supervisor = new BackendSupervisor({ onState: broadcast })
  createWindow()
  await supervisor.start()

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
})

app.on('window-all-closed', () => {
  // on plats other than macos, close window means the app should quit
  if (process.platform !== 'darwin') app.quit()
})

app.on('before-quit', async (event) => {
  if (!supervisor) return
  const pending = supervisor
  supervisor = null
  event.preventDefault()
  await pending.stop()
  app.quit()
})
