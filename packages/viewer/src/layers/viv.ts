import type { Layer } from '@deck.gl/core'

/**
 * viv's layer constructors are typed `new (...props) => any`, and the `Viv<>` prop type
 * drops every extension prop (`colors`, `gammas`): it adds them only when `LayerProps`
 * matches `{ extensions: unknown }`, which an optional `extensions?: any[]` never does.
 * Every viv layer this package builds goes through this one cast.
 */
export const vivLayer = <P extends object>(
  Ctor: new (...props: never[]) => unknown,
  props: P,
): Layer => new (Ctor as unknown as new (props: P) => Layer)(props)
