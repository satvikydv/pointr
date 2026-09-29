/**
 * Pointr Navigation & Interactive UI Controller
 * Ensures unified navbar behavior, mobile drawer support, and rich interactive showcases.
 */
document.addEventListener('DOMContentLoaded', () => {
  // ==========================================
  // 1. MOBILE DRAWER NAVIGATION
  // ==========================================
  const toggleBtn = document.querySelector('.mobile-toggle');
  const mobileDrawer = document.querySelector('.mobile-drawer');
  const mobileLinks = document.querySelectorAll('.mobile-nav-link');
  const desktopLinks = document.querySelectorAll('.nav-menu .nav-link');

  if (toggleBtn && mobileDrawer) {
    function toggleMenu(open) {
      const isOpen = open !== undefined ? open : !mobileDrawer.classList.contains('is-open');
      toggleBtn.classList.toggle('is-active', isOpen);
      mobileDrawer.classList.toggle('is-open', isOpen);
      toggleBtn.setAttribute('aria-expanded', isOpen ? 'true' : 'false');
      document.body.style.overflow = isOpen ? 'hidden' : '';
    }

    toggleBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      toggleMenu();
    });

    mobileLinks.forEach((link) => {
      link.addEventListener('click', () => {
        toggleMenu(false);
      });
    });

    document.addEventListener('click', (e) => {
      if (mobileDrawer.classList.contains('is-open') && !mobileDrawer.contains(e.target) && !toggleBtn.contains(e.target)) {
        toggleMenu(false);
      }
    });

    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && mobileDrawer.classList.contains('is-open')) {
        toggleMenu(false);
      }
    });
  }

  // ==========================================
  // 2. ACTIVE ANCHOR HIGHLIGHTING (INDEX.HTML)
  // ==========================================
  const isHomePage = window.location.pathname.endsWith('index.html') || window.location.pathname === '/' || window.location.pathname.endsWith('/landing/');
  if (isHomePage) {
    const sections = document.querySelectorAll('section[id], div[id="features"], div[id="changelog"]');
    
    function highlightOnScroll() {
      const scrollY = window.pageYOffset;
      sections.forEach((section) => {
        const sectionHeight = section.offsetHeight;
        const sectionTop = section.offsetTop - 140;
        const sectionId = section.getAttribute('id');
        
        if (scrollY > sectionTop && scrollY <= sectionTop + sectionHeight) {
          desktopLinks.forEach((link) => {
            if (link.getAttribute('href') === `#${sectionId}` || link.getAttribute('href') === `./index.html#${sectionId}`) {
              link.classList.add('active');
            } else if (!link.getAttribute('href').includes('.html')) {
              link.classList.remove('active');
            }
          });
        }
      });
      
      if (scrollY < 200) {
        desktopLinks.forEach((link) => {
          if (link.getAttribute('href').startsWith('#')) {
            link.classList.remove('active');
          }
        });
      }
    }

    window.addEventListener('scroll', highlightOnScroll, { passive: true });
  }

  // ==========================================
  // 3. INTERACTIVE VOICE SHOWCASE WIDGET
  // ==========================================
  const voiceCard = document.querySelector('.voice-interactive-card');
  if (voiceCard) {
    const micBtn = voiceCard.querySelector('.voice-mic-btn');
    const statusPill = voiceCard.querySelector('.voice-status-pill');
    const transcriptText = voiceCard.querySelector('.voice-transcript-text');
    const detectedTag = voiceCard.querySelector('.voice-detected-tag');
    const promptChips = voiceCard.querySelectorAll('.voice-chip');

    let activePrompt = 'Pointr, open Notepad and write the standup notes';
    let activeTag = 'Agent mode';
    let isSpeaking = false;
    let typeInterval = null;

    const prompts = {
      agent: {
        text: 'Pointr, open Notepad and write the standup notes',
        tag: 'Agent mode'
      },
      explain: {
        text: 'explain: how does the JWT token refresh flow work?',
        tag: 'Storyboard mode'
      },
      direct: {
        text: 'How do I center this SVG icon inside a flex container?',
        tag: 'Direct ask'
      }
    };

    promptChips.forEach((chip) => {
      chip.addEventListener('click', () => {
        promptChips.forEach(c => c.classList.remove('active'));
        chip.classList.add('active');
        const key = chip.dataset.type;
        if (prompts[key]) {
          activePrompt = prompts[key].text;
          activeTag = prompts[key].tag;
          if (!isSpeaking) {
            transcriptText.textContent = `"${activePrompt}"`;
            detectedTag.textContent = activeTag;
          }
        }
      });
    });

    function startSpeaking() {
      if (isSpeaking) return;
      isSpeaking = true;
      voiceCard.classList.add('is-listening');
      statusPill.classList.add('recording');
      statusPill.innerHTML = '<div class="live-dot" style="background:#ff6b6b;box-shadow:0 0 8px #ff6b6b;"></div> Listening on-device…';
      micBtn.innerHTML = '<span>🎙️ Release to Transcribe</span>';
      
      transcriptText.textContent = '';
      detectedTag.style.opacity = '0.3';
      detectedTag.textContent = 'Transcribing local audio…';

      let charIndex = 0;
      clearInterval(typeInterval);
      typeInterval = setInterval(() => {
        if (charIndex < activePrompt.length) {
          transcriptText.textContent += activePrompt[charIndex];
          charIndex++;
        }
      }, 35);
    }

    function stopSpeaking() {
      if (!isSpeaking) return;
      isSpeaking = false;
      clearInterval(typeInterval);
      transcriptText.textContent = `"${activePrompt}"`;
      voiceCard.classList.remove('is-listening');
      statusPill.classList.remove('recording');
      statusPill.innerHTML = '<div class="live-dot" style="background:#3fb59a;"></div> 0ms Local Transcribed';
      micBtn.innerHTML = '<span>🎙️ Hold or Click to Speak</span>';
      detectedTag.style.opacity = '1';
      detectedTag.textContent = `${activeTag} (NV-Parakeet)`;
    }

    if (micBtn) {
      micBtn.addEventListener('mousedown', startSpeaking);
      window.addEventListener('mouseup', () => {
        if (isSpeaking) stopSpeaking();
      });

      // Touch support for mobile
      micBtn.addEventListener('touchstart', (e) => {
        e.preventDefault();
        startSpeaking();
      });
      window.addEventListener('touchend', () => {
        if (isSpeaking) stopSpeaking();
      });
    }
  }

  // ==========================================
  // 4. INTERACTIVE PERMISSIONS SANDBOX (INDEX.HTML)
  // ==========================================
  const permCard = document.querySelector('.perm-interactive-card');
  if (permCard) {
    const tabBtns = permCard.querySelectorAll('.perm-tab-btn');
    const stateBox = permCard.querySelector('.perm-state-box');
    const confirmBtn = permCard.querySelector('.perm-btn-confirm');
    const cancelBtn = permCard.querySelector('.perm-btn-cancel');

    let currentMode = 'ask';
    let resetTimer = null;

    function renderMode(mode) {
      currentMode = mode;
      clearTimeout(resetTimer);

      if (mode === 'ask') {
        stateBox.innerHTML = `
          <div style="display:flex;align-items:center;gap:8px;">
            <div style="width:8px;height:8px;border-radius:50%;background:#f2b84b;box-shadow:0 0 8px rgba(242,184,75,0.6);"></div>
            <div style="font-size:11px;font-weight:700;letter-spacing:0.06em;color:#f2b84b;text-transform:uppercase;">Action requested (Protected)</div>
          </div>
          <div style="font-size:14.5px;font-weight:600;color:rgba(255,255,255,0.96);">Type into Slack - #eng-standup</div>
          <div style="background:rgba(0,0,0,0.35);border:1px solid rgba(242,184,75,0.2);border-radius:8px;padding:9px 12px;font-family:var(--font-mono);font-size:12px;line-height:1.5;color:rgba(255,255,255,0.85);">Fixed the unmount bug - added cleanup flag…</div>
          <div class="perm-action-btns">
            <button class="perm-btn-confirm" id="btnConfirmAction">
              <span class="kbd-key" style="font-size:10px;padding:1px 5px;">Enter</span> Confirm Action
            </button>
            <button class="perm-btn-cancel" id="btnCancelAction">
              <span class="kbd-key" style="font-size:10px;padding:1px 5px;">Esc</span> Cancel
            </button>
          </div>
        `;
        attachActionListeners();
      } else if (mode === 'allow') {
        stateBox.innerHTML = `
          <div style="display:flex;align-items:center;justify-content:space-between;">
            <div style="display:flex;align-items:center;gap:8px;">
              <div class="live-dot" style="background:#5B8CFF;"></div>
              <div style="font-size:11px;font-weight:700;letter-spacing:0.06em;color:#8fb4ff;text-transform:uppercase;">Autonomous Mode Active</div>
            </div>
            <span class="flow-badge blue" style="font-size:10px;">Zero confirmation pauses</span>
          </div>
          <div style="font-size:14px;color:rgba(255,255,255,0.9);">Pointr executes steps directly upon plan approval.</div>
          <div style="background:rgba(91,140,255,0.08);border:1px solid rgba(91,140,255,0.25);border-radius:8px;padding:10px 12px;display:flex;align-items:center;justify-content:space-between;">
            <div style="display:flex;align-items:center;gap:8px;font-size:12.5px;color:#dce8ff;">
              <span style="font-size:14px;">⚡</span> Step 2/3: Typing into Slack...
            </div>
            <span style="font-family:var(--font-mono);font-size:11px;color:#8fb4ff;">Running</span>
          </div>
          <div style="font-size:12px;color:var(--text-muted);display:flex;align-items:center;gap:6px;">
            <span>Emergency abort:</span> <span class="kbd-key" style="font-size:10px;padding:1px 5px;">Esc</span> stops execution at any instant
          </div>
        `;
      } else if (mode === 'off') {
        stateBox.innerHTML = `
          <div style="display:flex;align-items:center;gap:8px;">
            <div style="width:8px;height:8px;border-radius:50%;background:#888;"></div>
            <div style="font-size:11px;font-weight:700;letter-spacing:0.06em;color:#aaa;text-transform:uppercase;">Safety Lock Active (Read-Only)</div>
          </div>
          <div style="font-size:14px;color:rgba(255,255,255,0.85);">Pointr is strictly limited to answering questions & drawing on-screen annotations.</div>
          <div style="background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.08);border-radius:8px;padding:10px 12px;font-size:12.5px;color:var(--text-muted);">
            🛡️ It will never click, type, focus windows, or press keys on your computer.
          </div>
        `;
      }
    }

    function attachActionListeners() {
      const cBtn = document.getElementById('btnConfirmAction');
      const xBtn = document.getElementById('btnCancelAction');

      if (cBtn) {
        cBtn.addEventListener('click', () => {
          stateBox.innerHTML = `
            <div style="display:flex;flex-direction:column;align-items:center;justify-content:center;gap:8px;padding:16px 0;text-align:center;">
              <div style="width:36px;height:36px;border-radius:50%;background:rgba(63,181,154,0.18);border:1px solid #3fb59a;color:#3fb59a;display:flex;align-items:center;justify-content:center;font-size:18px;">✓</div>
              <div style="font-size:15px;font-weight:600;color:#fff;">Action Confirmed &amp; Dispatched</div>
              <div style="font-size:12.5px;color:var(--text-muted);">Typed standup note into Slack with explicit approval.</div>
            </div>
          `;
          resetTimer = setTimeout(() => renderMode('ask'), 2800);
        });
      }

      if (xBtn) {
        xBtn.addEventListener('click', () => {
          stateBox.innerHTML = `
            <div style="display:flex;flex-direction:column;align-items:center;justify-content:center;gap:8px;padding:16px 0;text-align:center;">
              <div style="width:36px;height:36px;border-radius:50%;background:rgba(255,107,107,0.15);border:1px solid #ff6b6b;color:#ff6b6b;display:flex;align-items:center;justify-content:center;font-size:18px;">✕</div>
              <div style="font-size:15px;font-weight:600;color:#fff;">Action Aborted Safely</div>
              <div style="font-size:12.5px;color:var(--text-muted);">Esc pressed: no keys or mouse events were sent.</div>
            </div>
          `;
          resetTimer = setTimeout(() => renderMode('ask'), 2800);
        });
      }
    }

    tabBtns.forEach((btn) => {
      btn.addEventListener('click', () => {
        tabBtns.forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        renderMode(btn.dataset.mode);
      });
    });

    renderMode('ask');
  }

  // ==========================================
  // 5. SETUP GUIDE ARCHITECTURE FLOWCHART TOGGLES
  // ==========================================
  const voiceFlowContainer = document.querySelector('#voiceFlowContainer');
  if (voiceFlowContainer) {
    const flowTabs = voiceFlowContainer.querySelectorAll('.flowchart-tab');
    const pipelineView = voiceFlowContainer.querySelector('.flow-pipeline');

    const pipelines = {
      local: `
        <div class="flow-step-box highlight">
          <span class="flow-badge blue">Input</span>
          <div style="font-size:14px;font-weight:600;color:#fff;">Push-to-Talk</div>
          <div style="font-size:12px;color:var(--text-secondary);">Hold <span class="kbd-key">Ctrl</span> + <span class="kbd-key">Win</span></div>
          <div style="font-size:11px;color:#8fb4ff;margin-top:2px;">Mic active only while held</div>
        </div>
        <div class="flow-arrow-icon">➔</div>
        <div class="flow-step-box green">
          <span class="flow-badge green">100% On-Device</span>
          <div style="font-size:14px;font-weight:600;color:#fff;">NVIDIA Parakeet</div>
          <div style="font-size:12px;color:var(--text-secondary);">Local 670 MB Speech Model</div>
          <div style="font-size:11px;color:#3fb59a;margin-top:2px;">Audio never leaves your PC</div>
        </div>
        <div class="flow-arrow-icon">➔</div>
        <div class="flow-step-box">
          <span class="flow-badge blue">Memory Clean</span>
          <div style="font-size:14px;font-weight:600;color:#fff;">Zero Disk Audio</div>
          <div style="font-size:12px;color:var(--text-secondary);">Audio discarded instantly</div>
          <div style="font-size:11px;color:var(--text-muted);margin-top:2px;">Only text prompt forwarded</div>
        </div>
      `,
      cloud: `
        <div class="flow-step-box highlight">
          <span class="flow-badge blue">Input</span>
          <div style="font-size:14px;font-weight:600;color:#fff;">Push-to-Talk</div>
          <div style="font-size:12px;color:var(--text-secondary);">Hold <span class="kbd-key">Ctrl</span> + <span class="kbd-key">Win</span></div>
          <div style="font-size:11px;color:#8fb4ff;margin-top:2px;">Mic streams on release</div>
        </div>
        <div class="flow-arrow-icon">➔</div>
        <div class="flow-step-box gold">
          <span class="flow-badge gold">Direct BYOK</span>
          <div style="font-size:14px;font-weight:600;color:#fff;">OpenAI Whisper</div>
          <div style="font-size:12px;color:var(--text-secondary);">Under your OpenAI API Key</div>
          <div style="font-size:11px;color:#f2b84b;margin-top:2px;">No 670 MB model download</div>
        </div>
        <div class="flow-arrow-icon">➔</div>
        <div class="flow-step-box">
          <span class="flow-badge blue">Direct Stream</span>
          <div style="font-size:14px;font-weight:600;color:#fff;">Text Handover</div>
          <div style="font-size:12px;color:var(--text-secondary);">Direct API connection</div>
          <div style="font-size:11px;color:var(--text-muted);margin-top:2px;">Billed to your OpenAI account</div>
        </div>
      `
    };

    flowTabs.forEach((tab) => {
      tab.addEventListener('click', () => {
        flowTabs.forEach(t => t.classList.remove('active'));
        tab.classList.add('active');
        pipelineView.innerHTML = pipelines[tab.dataset.flow] || pipelines.local;
      });
    });
  }

  // ==========================================
  // 6. SETUP GUIDE PERMISSIONS MATRIX CARDS
  // ==========================================
  const permCards = document.querySelectorAll('.perm-tier-card');
  permCards.forEach((card) => {
    card.addEventListener('click', () => {
      permCards.forEach(c => c.classList.remove('selected'));
      card.classList.add('selected');
    });
  });



  // Interactive pulse on clicking the central orb
  const heroOrb = document.querySelector('.hero-orb-sphere');
  if (heroOrb) {
    heroOrb.addEventListener('click', () => {
      heroOrb.style.transform = 'scale(1.12)';
      heroOrb.style.boxShadow = '0 0 100px rgba(91, 140, 255, 0.95), inset 0 0 40px rgba(255, 255, 255, 0.5)';
      setTimeout(() => {
        heroOrb.style.transform = '';
        heroOrb.style.boxShadow = '';
      }, 450);
    });
  }
});

