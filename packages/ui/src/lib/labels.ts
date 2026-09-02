import type {
  CellRow,
  LabelDefinition,
  LabelDefinitionInput,
  LabelScope,
  LabelState,
  SetLabelsBody,
} from '@cellstudio/api-client'
import { defaultLabelColor } from '@cellstudio/viewer'

/** Trimmed name, or the reason it cannot be added. */
export function validateLabelName(
  raw: string,
  existing: readonly LabelDefinition[],
): { name: string } | { error: string } {
  const name = raw.trim()
  if (name.length === 0) return { error: 'A label needs a name' }
  if (existing.some((d) => d.name === name)) return { error: `"${name}" is already defined` }
  return { name }
}

/** The stored list as `PUT` takes it: every current entry with its colour. */
export const toInputs = (existing: readonly LabelDefinition[]): LabelDefinitionInput[] =>
  existing.map((d) => ({ name: d.name, color: d.color ?? null }))

/** The stored list after adding `name` with a name-derived default colour, sorted. */
export function withDefinition(
  existing: readonly LabelDefinition[],
  name: string,
): LabelDefinitionInput[] {
  return [...toInputs(existing), { name, color: defaultLabelColor(name) }].sort((a, b) =>
    a.name.localeCompare(b.name),
  )
}

/** The stored list with one entry's colour replaced. */
export function withColor(
  existing: readonly LabelDefinition[],
  name: string,
  color: string,
): LabelDefinitionInput[] {
  return toInputs(existing).map((d) => (d.name === name ? { ...d, color } : d))
}

/** One checkbox click: a fully-applied label clears, anything else applies. */
export function toggleBody(state: LabelState, scope: LabelScope, cellId: number): SetLabelsBody {
  const on = scope === 'cell' ? state.cell : state.track === 'all'
  return on ? { cellId, scope, remove: [state.name] } : { cellId, scope, add: [state.name] }
}

/**
 * First-paint states from the selected row alone, before the chain query answers: a
 * track-scope tag the row carries reads as `all`, which the server may refine to `some`.
 */
export function statesFromRow(
  definitions: readonly LabelDefinition[],
  row: CellRow | null,
): LabelState[] {
  return definitions.map((d) => ({
    name: d.name,
    cell: row?.labels.includes(d.name) ?? false,
    track: row?.trackLabels.includes(d.name) ? 'all' : 'none',
  }))
}

/** Tooltip for a sheet row's remove control. */
export function removeHint(def: LabelDefinition): string {
  if (def.uses === 0) return `Remove "${def.name}"`
  const cells = def.uses === 1 ? '1 cell' : `${def.uses} cells`
  return `Remove "${def.name}" from ${cells} (undoable)`
}
