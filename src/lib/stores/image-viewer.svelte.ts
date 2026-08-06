// Global image lightbox state. One <ImageLightbox> instance (mounted in +layout.svelte)
// reads this; any component can open the viewer by calling imageViewer.openOne/openMany.
// Holds a list + index so a message with several images can be paged in place.

export interface ViewerImage {
  /** A displayable src — data URL, blob:, or resolved absolute URL. */
  src: string;
  name?: string;
  type?: string;
}

let images = $state<ViewerImage[]>([]);
let index = $state(0);
let isOpen = $state(false);

export const imageViewer = {
  get open() {
    return isOpen;
  },
  get images() {
    return images;
  },
  get index() {
    return index;
  },
  get current(): ViewerImage | undefined {
    return images[index];
  },

  /** Open the viewer on a single image. */
  openOne(img: ViewerImage) {
    if (!img?.src) return;
    images = [img];
    index = 0;
    isOpen = true;
  },

  /** Open the viewer on a list, starting at `startIndex` (clamped). */
  openMany(list: ViewerImage[], startIndex = 0) {
    if (!list?.length) return;
    images = list;
    index = Math.max(0, Math.min(startIndex, list.length - 1));
    isOpen = true;
  },

  next() {
    if (images.length > 1) index = (index + 1) % images.length;
  },

  prev() {
    if (images.length > 1) index = (index - 1 + images.length) % images.length;
  },

  close() {
    isOpen = false;
    images = [];
    index = 0;
  },
};
