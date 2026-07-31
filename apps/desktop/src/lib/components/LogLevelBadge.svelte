<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!--
  The ONE level → colour renderer for a log line (spec D6: "the row renderer
  is extracted so level colours cannot drift between the two surfaces").
  Consumed by `LogPane.svelte` (ring rows, `ServiceLogEvent`'s `LogLevel`)
  and `LogBody.svelte` (file rows, `LogRowDto.level`) — both reuse the same
  `LogLevel` type (`openvhost_proc::LogLevel`, spec D4's doc comment on why
  it is shared rather than duplicated) and now the same markup/colour
  mapping, so the two surfaces cannot quietly disagree on what "warn" looks
  like the way two independently-hand-rolled `levelClass` copies could.
-->
<script lang="ts">
	import type { LogLevel } from '../ipc';
	import { levelClass } from '../logs.derive';

	let { level }: { level: LogLevel } = $props();
</script>

<span class="lvl {levelClass(level)}">{level}</span>

<style>
	/* Ported from docs/design/mock.css (.log .lvl, .lvl-info/.lvl-warn/.lvl-error) —
	   identical to the rule LogPane.svelte carried inline before this extraction. */
	.lvl {
		font-weight: 700;
	}
	.lvl-info {
		color: var(--vh-text-2);
	}
	.lvl-warn {
		color: var(--vh-start);
	}
	.lvl-error {
		color: var(--vh-fail);
	}
</style>
