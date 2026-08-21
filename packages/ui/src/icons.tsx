import type { Tool } from '@cellstudio/viewer'
import type { ReactElement } from 'react'

const box = { viewBox: '0 0 24 24' } as const

export const MarkIcon = () => (
  <svg {...box}>
    <circle cx="8" cy="9" r="3" />
    <circle cx="15.5" cy="14.5" r="3.5" />
    <path d="M10 11l3 2" />
  </svg>
)

export const TOOL_ICONS: Record<Tool, () => ReactElement> = {
  pointer: () => (
    <svg {...box}>
      <path d="M5 3l13 9-6 1.5L9 20z" />
    </svg>
  ),
  pan: () => (
    <svg {...box}>
      <path d="M8 11V7a1.5 1.5 0 013 0v3-5a1.5 1.5 0 013 0v5-3a1.5 1.5 0 013 0v4-2a1.5 1.5 0 013 0v5c0 4-2.5 7-7 7h-1c-2.7 0-4.6-1.8-6-4l-2-3a1.6 1.6 0 012.5-2l1.5 1.5V8a1.5 1.5 0 013 0v3" />
    </svg>
  ),
  brush: () => (
    <svg {...box}>
      <path d="M14 4l6 6-9 9H5v-6z" />
      <path d="M13 5l2-2 6 6-2 2M5 19H3" />
    </svg>
  ),
  eraser: () => (
    <svg {...box}>
      <path d="M4 15l9-11 7 6-8 10H8zM12 20h9" />
    </svg>
  ),
  fill: () => (
    <svg {...box}>
      <path d="M4 14l8-9 7 7-8 8H4zM9 8l7 7M18 17s-2 2.2-2 3.2a2 2 0 004 0C20 19.2 18 17 18 17z" />
    </svg>
  ),
  pick: () => (
    <svg {...box}>
      <path d="M14 4l6 6-9 9H5v-6zM15 5l2-2 4 4-2 2" />
      <circle cx="7" cy="17" r="1" />
    </svg>
  ),
  link: () => (
    <svg {...box}>
      <path d="M10 13a4 4 0 005.7.1l2-2a4 4 0 00-5.7-5.7l-1.1 1.1M14 11a4 4 0 00-5.7-.1l-2 2A4 4 0 0012 18.6l1.1-1.1" />
    </svg>
  ),
  cut: () => (
    <svg {...box}>
      <path d="M8.5 8.5L6 11a4 4 0 005.7 5.7l1-1M15.5 15.5L18 13a4 4 0 00-5.7-5.7l-1 1M4 4l16 16" />
    </svg>
  ),
}

export const ResetViewIcon = () => (
  <svg {...box}>
    <path d="M19 12a7 7 0 11-2.05-4.95M17 3v4h-4" />
  </svg>
)

export const HelpIcon = () => (
  <svg {...box}>
    <circle cx="12" cy="12" r="9" />
    <path d="M9.8 9a2.3 2.3 0 114 1.6c-1.2.8-1.8 1.2-1.8 2.4M12 17h.01" />
  </svg>
)

export const GearIcon = () => (
  <svg {...box}>
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.6 1.6 0 00.3 1.8l.1.1-2.8 2.8-.1-.1a1.6 1.6 0 00-1.8-.3 1.6 1.6 0 00-1 1.5V21h-4v-.2a1.6 1.6 0 00-1-1.5 1.6 1.6 0 00-1.8.3l-.1.1-2.8-2.8.1-.1a1.6 1.6 0 00.3-1.8 1.6 1.6 0 00-1.5-1H3v-4h.2a1.6 1.6 0 001.5-1 1.6 1.6 0 00-.3-1.8l-.1-.1 2.8-2.8.1.1a1.6 1.6 0 001.8.3 1.6 1.6 0 001-1.5V3h4v.2a1.6 1.6 0 001 1.5 1.6 1.6 0 001.8-.3l.1-.1 2.8 2.8-.1.1a1.6 1.6 0 00-.3 1.8 1.6 1.6 0 001.5 1h.2v4h-.2a1.6 1.6 0 00-1.5 1z" />
  </svg>
)

export const PlayIcon = () => (
  <svg {...box}>
    <path d="M8 5l11 7-11 7z" />
  </svg>
)

export const PauseIcon = () => (
  <svg {...box}>
    <path d="M8 6v12M16 6v12" />
  </svg>
)

export const CheckIcon = () => (
  <svg viewBox="0 0 12 12">
    <path d="M2 6l2.4 2.4L10 3" />
  </svg>
)

export const AlertIcon = () => (
  <svg {...box}>
    <path d="M12 3l9 17H3zM12 9v5M12 17h.01" />
  </svg>
)
