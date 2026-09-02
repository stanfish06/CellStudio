import type {
  Bbox,
  CellRow,
  DeleteMaskBody,
  EditResult,
  GraphEditResult,
  LabelDefinitionInput,
  LabelDefinitionsResult,
  LabelLease,
  LayerId,
  MaskEditResult,
  PlaneBuffer,
  SetLabelsBody,
  StrokeBody,
  VolumeBuffer,
} from '@cellstudio/api-client'

export type OrthoAxis = 'xy' | 'xz' | 'yz'

/**
 * The subset of `ApiClient` the viewer's pixel plane uses.
 */
export interface PixelApi {
  slice(
    q: { layer: LayerId; axis: OrthoAxis; t: number; cs: number[]; pos: number; level: number },
    signal?: AbortSignal,
  ): Promise<PlaneBuffer>
  volume(
    q: { layer: LayerId; t: number; c: number; level: number },
    signal?: AbortSignal,
  ): Promise<VolumeBuffer>
  pixel(
    q: { layer: LayerId; t: number; c: number; z: number; y: number; x: number },
    signal?: AbortSignal,
  ): Promise<number>
  cellsWindow(q: { t0: number; t1: number; bbox?: Bbox }, signal?: AbortSignal): Promise<CellRow[]>
}

/** Graph mutations; the session posts these and routes the result to `advanceGraph`. */
export interface GraphApi {
  link(body: { parentId: number; childId: number }): Promise<GraphEditResult>
  unlink(body: { cellId: number }): Promise<GraphEditResult>
  cut(body: { parentId: number; childId: number }): Promise<GraphEditResult>
  setLabels(body: SetLabelsBody): Promise<GraphEditResult>
  putLabelDefinitions(definitions: LabelDefinitionInput[]): Promise<LabelDefinitionsResult>
  /** The strip edit it may carry routes to `advanceGraph` like any other. */
  deleteLabelDefinition(name: string): Promise<LabelDefinitionsResult>
}

/** The mask write path; `MaskEditor` holds this, the scenes do not. */
export interface MaskApi {
  reserveLabels(count: number): Promise<LabelLease>
  stroke(body: StrokeBody): Promise<MaskEditResult>
  deleteMask(body: DeleteMaskBody): Promise<MaskEditResult>
  undo(): Promise<EditResult>
  redo(): Promise<EditResult>
}
