import { describe, expect, it } from 'vitest'
import { emptyDownloadProgress, formatBytes, updateDownloadProgress } from './update-progress'

describe('desktop bootstrap contract', () => {
  it('accepts only the local Harness origin', () => {
    const accepted = new URL('http://127.0.0.1:3080')
    expect(accepted.protocol).toBe('http:')
    expect(accepted.hostname).toBe('127.0.0.1')
  })
})

describe('updater progress', () => {
  it('calculates bounded download progress', () => {
    const started = updateDownloadProgress(emptyDownloadProgress, {
      event: 'Started',
      data: { contentLength: 100 },
    })
    const downloaded = updateDownloadProgress(started, {
      event: 'Progress',
      data: { chunkLength: 45 },
    })
    const overReported = updateDownloadProgress(downloaded, {
      event: 'Progress',
      data: { chunkLength: 80 },
    })

    expect(downloaded.percent).toBe(45)
    expect(overReported.percent).toBe(100)
    expect(updateDownloadProgress(overReported, { event: 'Finished' }).finished).toBe(true)
  })

  it('formats download sizes for the update screen', () => {
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(1536)).toBe('1.5 KB')
    expect(formatBytes(2 * 1024 * 1024)).toBe('2.0 MB')
  })
})
