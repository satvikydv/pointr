/**
 * Pointr Navigation & Interactive UI Controller
 * Ensures unified navbar behavior, mobile drawer support, and smooth page interactions.
 */
document.addEventListener('DOMContentLoaded', () => {
  const toggleBtn = document.querySelector('.mobile-toggle');
  const mobileDrawer = document.querySelector('.mobile-drawer');
  const mobileLinks = document.querySelectorAll('.mobile-nav-link');
  const desktopLinks = document.querySelectorAll('.nav-menu .nav-link');

  // Toggle mobile drawer
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

    // Close when clicking mobile links
    mobileLinks.forEach((link) => {
      link.addEventListener('click', () => {
        toggleMenu(false);
      });
    });

    // Close when clicking outside
    document.addEventListener('click', (e) => {
      if (mobileDrawer.classList.contains('is-open') && !mobileDrawer.contains(e.target) && !toggleBtn.contains(e.target)) {
        toggleMenu(false);
      }
    });

    // Close on Escape key
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && mobileDrawer.classList.contains('is-open')) {
        toggleMenu(false);
      }
    });
  }

  // Active anchor highlighting on index.html
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
      
      // If at top of page, remove active from anchor links
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
});
