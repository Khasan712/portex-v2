function copyToClipboard(text, button) {
        navigator.clipboard
          .writeText(text)
          .then(() => {
            button.classList.add('copied');
            setTimeout(() => button.classList.remove('copied'), 150);
          })
          .catch((err) => {
            alert('Failed to copy: ' + err);
          });
      }