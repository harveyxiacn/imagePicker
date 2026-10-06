import type { ColorName } from '@/api/types'

export const COLOR_NAMES: ColorName[] = ['red', 'yellow', 'green', 'blue', 'purple']

export const colorVar = (c: ColorName) => `var(--label-${c})`
