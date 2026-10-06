import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import './i18n'
import { App } from './App'

async function bootstrap() {
  if (__MOCK__) {
    const { enableMock } = await import('./mock')
    await enableMock()
  }
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  )
}

void bootstrap()
