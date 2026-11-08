<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Share a station, a track on air or a list (docs/16-sharing.md §4): the link to copy, its QR
     code to send it from the computer to a phone, and what leaves the device. -->
<script lang="ts">
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import { errorMessage } from '../api';
  import { share } from '../share.svelte';
  import Icon from './Icon.svelte';
  import sheet from './StationSheet.module.css';
  import styles from './SharePanel.module.css';

  let dialog: HTMLDialogElement | undefined = $state();
  let copied = $state(false);
  let copyError = $state<string | null>(null);

  const offer = $derived(share.offer);
  /** At least 4 px per module, so that a phone reads it from a screen (§4.1). */
  const qrSize = $derived(offer ? Math.max(160, offer.out.qr_modules * 4) : 0);

  $effect(() => {
    if (!dialog) return;
    if ((offer || share.offerError) && !dialog.open) {
      copied = false;
      copyError = null;
      dialog.showModal();
    } else if (!offer && !share.offerError && dialog.open) {
      dialog.close();
    }
  });

  /** What the link tells about stations added by hand, when it carries or looked up any. */
  const notice = $derived.by(() => {
    if (!offer) return null;
    const { addresses, looked_up, item } = offer.out;
    if (addresses > 0) {
      return looked_up > 0
        ? 'Looked up in Radio-Browser by its stream address, and not found there: the stream address goes in the link.'
        : addresses === 1
          ? 'The stream address goes in the link.'
          : `The addresses of ${addresses} stations added by hand go in the link.`;
    }
    if (looked_up > 0 && item.kind === 'station') {
      return 'Found in Radio-Browser by its stream address: the link names the catalog station, without your address.';
    }
    return null;
  });

  async function copy() {
    if (!offer) return;
    copyError = null;
    try {
      await writeText(offer.out.url);
      copied = true;
    } catch (e) {
      copyError = errorMessage(e);
    }
  }
</script>

<dialog
  class={`${sheet.sheet} ${styles.panel}`}
  bind:this={dialog}
  aria-labelledby="share-panel-title"
  onclose={share.closeOffer}
  onclick={(e) => e.target === dialog && share.closeOffer()}
>
  <header class={sheet.header}>
    <h2 class={sheet.title} id="share-panel-title">
      {offer ? `Share ${offer.name}` : 'Share'}
    </h2>
    <button class={sheet.close} type="button" aria-label="Close" onclick={share.closeOffer}
      ><Icon name="x" /></button
    >
  </header>

  {#if offer}
    <div class={styles.body}>
      {#if offer.track}<p class={sheet.text}>With the track on air: {offer.track}</p>{/if}
      <div class={styles.linkRow}>
        <input
          class={`${sheet.input} ${sheet.mono}`}
          type="text"
          readonly
          value={offer.out.url}
          aria-label="Link"
          onfocus={(e) => e.currentTarget.select()}
        />
        <button class={sheet.primary} type="button" onclick={copy}>
          {#if copied}<Icon name="check" size={14} /> Link copied{:else}Copy link{/if}
        </button>
      </div>
      {#if copyError}<p class={sheet.warning} role="alert">{copyError}</p>{/if}
      {#if notice}<p class={sheet.hint}>{notice}</p>{/if}
      {#if offer.out.qr}
        <figure class={styles.qr}>
          <img src={offer.out.qr} width={qrSize} height={qrSize} alt="QR code of the link" />
          <figcaption class={sheet.hint}>Scan it to open the link on a phone.</figcaption>
        </figure>
      {/if}
    </div>
  {:else if share.offerError}
    <p class={`${sheet.warning} ${styles.body}`} role="alert">{share.offerError}</p>
  {/if}
</dialog>
