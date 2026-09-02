import { pixelFromSliceWorld, type PixelZYX, type WorldTransform } from '../data/world'
import type { SliceOrientation } from '../state/nav'
import { Emitter } from './types'

/**
 * Matches `CursorSample` in `@cellstudio/ui`, which renders it in the status bar.
 * Coordinates are dataset pixels; display scaling divides out before this.
 */
export interface CursorSample {
  z: number
  y: number
  x: number
}

/**
 * The pointer's dataset position, updated on the move itself. Values under the cursor
 * (intensity, cell, track) belong to the Inspect tab's selection, not to hover.
 */
export class CursorReadout {
  private readonly changed = new Emitter()
  private current: CursorSample | null = null

  get sample(): CursorSample | null {
    return this.current
  }

  onChange(cb: () => void): () => void {
    return this.changed.on(cb)
  }

  /**
   * An explicit sample point in dataset pixels, floored to the voxel that contains it —
   * a slice hover, or the 3D orb centre while a paint tool is active.
   */
  move(sample: PixelZYX): void {
    const next = {
      z: Math.floor(sample[0]),
      y: Math.floor(sample[1]),
      x: Math.floor(sample[2]),
    }
    const same =
      this.current !== null &&
      this.current.z === next.z &&
      this.current.y === next.y &&
      this.current.x === next.x
    if (same) return
    this.current = next
    this.changed.emit()
  }

  /** Pointer position on a slice quad, converted through the shared world transform. */
  moveOnSlice(
    world: readonly [number, number],
    ctx: { orientation: SliceOrientation; index: number; transform: WorldTransform },
  ): void {
    const pixel = pixelFromSliceWorld(ctx.orientation, ctx.index, world, ctx.transform)
    this.move([Math.round(pixel[0]), Math.floor(pixel[1]), Math.floor(pixel[2])])
  }

  clear(): void {
    if (this.current === null) return
    this.current = null
    this.changed.emit()
  }

  dispose(): void {
    this.current = null
    this.changed.clear()
  }
}
