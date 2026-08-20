import type { DownloadEvent } from '@tauri-apps/plugin-updater'

export interface DownloadProgress {
  downloadedBytes: number
  contentLength?: number
  percent?: number
  finished: boolean
}

export const emptyDownloadProgress: DownloadProgress = {
  downloadedBytes: 0,
  finished: false,
}

export function updateDownloadProgress(
  current: DownloadProgress,
  event: DownloadEvent,
): DownloadProgress {
  if (event.event === 'Started') {
    return {
      downloadedBytes: 0,
      contentLength: event.data.contentLength,
      percent: event.data.contentLength ? 0 : undefined,
      finished: false,
    }
  }

  if (event.event === 'Progress') {
    const downloadedBytes = current.downloadedBytes + event.data.chunkLength
    return {
      ...current,
      downloadedBytes,
      percent: current.contentLength
        ? Math.min(100, Math.round((downloadedBytes / current.contentLength) * 100))
        : undefined,
    }
  }

  return {
    ...current,
    percent: current.contentLength ? 100 : current.percent,
    finished: true,
  }
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}
