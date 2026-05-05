<script setup lang="ts">
// Skeleton page. Subsequent issues populate the views:
//   #151 — JS-native .dop loader (Buf-generated TS types + decimal.js)
//   #150 — balance / register / chart Vue components
//   #149 — drag-and-drop WASM source-parser shim (post-1.0)
</script>

<template>
  <header class="hero">
    <h1>doppio</h1>
    <p class="tagline">
      A typed compiler pipeline for
      <a href="https://ledger-cli.org/" target="_blank" rel="noopener">Ledger</a>
      plain-text accounting. This page is a browser demo for the
      <code>.dop</code> compiled-journal format.
    </p>
  </header>

  <main class="content">
    <section class="placeholder">
      <h2>Coming soon</h2>
      <p>
        The interactive views land in
        <a href="https://github.com/alevy/doppio/issues/150" target="_blank" rel="noopener">#150</a>.
        The JS-native <code>.dop</code> reader (the central architectural
        validator of the format-as-API claim) lands in
        <a href="https://github.com/alevy/doppio/issues/151" target="_blank" rel="noopener">#151</a>.
      </p>
      <p>
        Until then, this page proves the build pipeline works end to end:
        <code>buf generate</code> from
        <code>proto/doppio.proto</code> → Vite/Vue static bundle → GitHub Pages.
      </p>
    </section>
  </main>

  <footer class="footer">
    <p>
      Source:
      <a href="https://github.com/alevy/doppio" target="_blank" rel="noopener">
        github.com/alevy/doppio
      </a>
      ·
      Schema:
      <a
        href="https://github.com/alevy/doppio/blob/main/proto/doppio.proto"
        target="_blank"
        rel="noopener"
      >
        proto/doppio.proto
      </a>
    </p>
  </footer>
</template>

<style scoped>
.hero {
  padding: 2.5rem 1.5rem 1.5rem;
  text-align: center;
  border-bottom: 1px solid #e5e5e5;
  background: #fff;
}

.hero h1 {
  margin: 0 0 0.5rem;
  font-size: 2.4rem;
  letter-spacing: -0.02em;
}

.tagline {
  margin: 0 auto;
  max-width: 40rem;
  color: #555;
}

.content {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 2rem 1.5rem;
}

.placeholder {
  max-width: 36rem;
  background: #fff;
  border: 1px solid #e5e5e5;
  border-radius: 0.5rem;
  padding: 1.5rem 2rem;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
}

.placeholder h2 {
  margin-top: 0;
  font-size: 1.2rem;
}

.footer {
  border-top: 1px solid #e5e5e5;
  padding: 1rem 1.5rem;
  text-align: center;
  color: #666;
  font-size: 0.9rem;
  background: #fff;
}
</style>
