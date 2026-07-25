<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!--
  Brand marks for the web servers OpenVHost can supervise.

  THIRD-PARTY ARTWORK — source and licence, per artwork:

  * nginx      — https://thesvg.org/icon/nginx  (variant: default)
                 Licence: CC0-1.0 (public-domain dedication).
  * Apache     — https://thesvg.org/icon/apache (variant: default)
                 Licence: Apache-2.0. Apache-2.0 is one-way compatible with
                 GPL-3.0-or-later, so it may be combined into this project;
                 this comment preserves the attribution notice the licence
                 requires. See https://github.com/glincker/thesvg.

  Both licences are already on this repo's `deny.toml` allow-list.

  TRADEMARKS: these marks remain trademarks of their respective owners — the
  nginx mark of F5, Inc., the feather of The Apache Software Foundation. They
  are used here nominatively, only to identify the third-party software a site
  is configured to run under. That is not sponsorship or endorsement. If either
  owner's brand policy ever conflicts with this use, drop back to the neutral
  glyphs the rail already draws by hand.

  Path data is verbatim from the sources above; only the XML prolog, generator
  comments and <title> elements were stripped, `style="fill:…"` was rewritten as
  a `fill` attribute, and sizing was moved to CSS. Each mark keeps its own
  viewBox — they differ (32 vs 24) and must not be normalised by hand.
-->
<script lang="ts">
	import type { WebServerKind } from '$lib/sites.derive';

	// `aria-hidden` throughout: every call site is a control that already names the
	// server in visible text, so announcing the mark again would just be noise.
	let { server }: { server: WebServerKind } = $props();
</script>

{#if server === 'nginx'}
	<svg class="brand" viewBox="0 0 32 32" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
		<path
			fill="#019639"
			d="M15.948,2h.065a10.418,10.418,0,0,1,.972.528Q22.414,5.65,27.843,8.774a.792.792,0,0,1,.414.788c-.008,4.389,0,8.777-.005,13.164a.813.813,0,0,1-.356.507q-5.773,3.324-11.547,6.644a.587.587,0,0,1-.657.037Q9.912,26.6,4.143,23.274a.7.7,0,0,1-.4-.666q0-6.582,0-13.163a.693.693,0,0,1,.387-.67Q9.552,5.657,14.974,2.535c.322-.184.638-.379.974-.535"
		/>
		<path
			fill="#fff"
			d="M8.767,10.538q0,5.429,0,10.859a1.509,1.509,0,0,0,.427,1.087,1.647,1.647,0,0,0,2.06.206,1.564,1.564,0,0,0,.685-1.293c0-2.62-.005-5.24,0-7.86q3.583,4.29,7.181,8.568a2.833,2.833,0,0,0,2.6.782,1.561,1.561,0,0,0,1.251-1.371q.008-5.541,0-11.081a1.582,1.582,0,0,0-3.152,0c0,2.662-.016,5.321,0,7.982-2.346-2.766-4.663-5.556-7-8.332A2.817,2.817,0,0,0,10.17,9.033,1.579,1.579,0,0,0,8.767,10.538Z"
		/>
	</svg>
{:else}
	<svg class="brand" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
		<path
			fill="#D22128"
			d="M17.805 2.197v.066h.156v.44h.072v-.44h.156v-.066zm.9 0l-.175.353-.172-.353h-.087v.506h.067V2.3l.172.35h.045l.172-.35v.404h.066v-.506zm-4.257 1c-.204.31-.424.66-.66 1.06l-.04.062a44.457 44.457 0 00-1.265 2.29c-.187.36-.38.742-.577 1.146l2.267-.25c.66-.302.955-.578 1.242-.976a15.5 15.5 0 00.23-.342c.23-.363.46-.763.663-1.16.197-.386.37-.767.505-1.11.083-.22.15-.422.198-.6.042-.158.074-.307.1-.45-.884.15-1.965.295-2.668.33zM11.894 7.78l-.077.16c-.078.16-.157.32-.236.488-.086.18-.172.364-.26.552l-.132.287a75.265 75.265 0 00-1.427 3.3c-.163.397-.327.807-.493 1.23-.15.38-.297.765-.45 1.164l-.02.06c-.15.396-.3.802-.453 1.22l-.01.027.72-.08a.213.213 0 01-.042-.006c.863-.106 2.01-.75 2.75-1.547.342-.367.652-.8.94-1.306.213-.377.413-.795.604-1.258.168-.405.328-.843.48-1.318-.196.105-.423.18-.673.235a2.184 2.184 0 01-.273.046c.806-.31 1.314-.905 1.683-1.64a2.816 2.816 0 01-.968.428c-.06.012-.116.022-.174.03l-.043.006h.002c.278-.118.514-.248.718-.403a2.571 2.571 0 00.637-.698l.063-.104.077-.154a8.107 8.107 0 00.367-.85l.03-.088a3.04 3.04 0 00.123-.463.733.733 0 01-.094.065c-.243.145-.66.277-.996.34l.663-.074-.664.073h-.017l-.1.017c.006-.003.01-.006.017-.008l-2.265.25-.013.022zM8.27 16.45c-.117.323-.236.654-.355.992l-.005.015c-.016.046-.032.094-.05.142-.08.227-.15.432-.31.9.264.12.475.435.675.793a1.44 1.44 0 00-.466-.99c1.293.06 2.41-.27 2.99-1.217.05-.084.096-.173.14-.268-.26.333-.59.474-1.2.44 0 0-.004 0-.005.002l.004-.002c.9-.404 1.354-.79 1.754-1.433.094-.153.186-.32.28-.503-.788.81-1.702 1.04-2.664.865l-.72.078a6.43 6.43 0 00-.067.183zM15.42.112c-.376.222-1 .85-1.748 1.763l.686 1.294c.48-.687.97-1.307 1.462-1.836l.058-.062c-.02.02-.04.04-.057.062-.16.176-.644.74-1.375 1.863.703-.035 1.784-.18 2.666-.33.262-1.47-.258-2.142-.258-2.142s-.66-1.07-1.436-.61zm-3.084 6.402a40.253 40.253 0 011.306-2.26l.04-.064c.224-.352.45-.693.677-1.02l-.685-1.293-.157.192c-.197.245-.403.51-.613.79a39.853 39.853 0 00-2.016 2.97l-.022.038.893 1.763c.19-.378.38-.752.575-1.118zm-3.73 8.32c.158-.406.319-.81.483-1.225.156-.394.32-.79.484-1.19a91.133 91.133 0 011.6-3.604l.205-.424c.12-.243.237-.485.36-.724a.125.125 0 01.02-.04l-.895-1.763-.044.07c-.207.34-.414.687-.617 1.042a38.056 38.056 0 00-1.092 2.04l-.094.193a24.573 24.573 0 00-1.258 3.087 18.492 18.492 0 00-.52 1.997l.896 1.77c.117-.317.24-.638.364-.963zm-1.376-.476a13.38 13.38 0 00-.234 1.692c0 .02-.004.04-.005.06-.28-.45-1.03-.888-1.026-.884.537.778.944 1.55 1.005 2.31-.29.058-.684-.027-1.14-.195.475.436.83.556.97.588-.434.03-.89.328-1.346.67.668-.27 1.21-.38 1.596-.29-.61 1.74-1.23 3.655-1.843 5.69a.538.538 0 00.364-.354c.11-.368.84-2.786 1.978-5.965l.097-.27.028-.078c.12-.332.246-.672.374-1.02l.09-.237v-.004L7.24 14.3c-.003.02-.01.04-.012.06z"
		/>
	</svg>
{/if}

<style>
	/* Sized in em so the mark tracks whatever type scale the host control uses, and
	   `flex: none` so a flex parent cannot squash a non-text child. */
	.brand {
		width: 1.15em;
		height: 1.15em;
		flex: none;
		display: block;
	}
</style>
