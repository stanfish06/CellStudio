import { MarkIcon } from '../icons'
import { basename } from '../lib/format'

export interface TitleBarProps {
  projectPath: string | null
}

export function TitleBar({ projectPath }: TitleBarProps) {
  return (
    <header className="titlebar">
      <span className="mark" aria-hidden="true">
        <MarkIcon />
      </span>
      <span className="project-title">
        {projectPath ? basename(projectPath) : 'No project open'}
      </span>
    </header>
  )
}
