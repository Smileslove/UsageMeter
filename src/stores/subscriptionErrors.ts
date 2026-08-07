import type { SubscriptionQueryResult } from '../types'

export function failedSubscriptionQuery(error: unknown): SubscriptionQueryResult {
  const message = error instanceof Error ? error.message : String(error)
  return {
    success: false,
    credentialStatus: { queryFailed: { error: message } },
    error: message,
    queriedAt: Date.now()
  }
}
