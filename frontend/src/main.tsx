import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
// flag-icons MUST load before our own styles so theme.css can override the
// .fi sizing for the pill wrapper.
import 'flag-icons/css/flag-icons.min.css';
// Self-hosted Noto Sans SC (思源黑体, OFL). Sharded woff2 + unicode-range:
// the browser only fetches the glyph blocks each page uses. 400 = body text,
// 500 = headings/menu. font-display: swap, so text shows in the system font
// first, then swaps in without blocking. Clean, well-hinted, reads crisp at
// small sizes — unlike MiSans which rendered heavy/blurry on Windows.
import '@fontsource/noto-sans-sc/400.css';
import '@fontsource/noto-sans-sc/500.css';
import './index.css'
import App from './App.tsx'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
