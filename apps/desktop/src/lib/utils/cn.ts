// SPDX-License-Identifier: GPL-3.0-or-later
import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

/** shadcn-svelte class combiner: clsx semantics + Tailwind conflict resolution. */
export function cn(...inputs: ClassValue[]): string {
	return twMerge(clsx(inputs));
}
