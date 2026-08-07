import { invoke } from '@tauri-apps/api/core'

/**
 * Invoke a Tauri command with a bounded wait time.
 *
 * The store should receive a normal Error instance so callers keep a useful
 * stack and can map the stable error code through i18n.
 */
export function invokeWithTimeout<T>(
  command: string,
  args: Record<string, unknown>,
  timeoutMs = 120000
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | null = null
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error('ERR_STATISTICS_TIMEOUT')), timeoutMs)
  })

  return Promise.race([invoke<T>(command, args), timeout]).finally(() => {
    if (timer) clearTimeout(timer)
  })
}
