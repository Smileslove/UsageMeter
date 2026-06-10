import { invoke } from '@tauri-apps/api/core'

interface AppExitLifecycle {
  stopAutoRefresh: () => void
  prepareExit: () => Promise<void>
}

export async function quitApplication(lifecycle: AppExitLifecycle): Promise<void> {
  lifecycle.stopAutoRefresh()
  await lifecycle.prepareExit()
  await invoke('confirm_exit')
}
