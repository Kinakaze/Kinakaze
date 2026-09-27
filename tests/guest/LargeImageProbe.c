/* Exercise large immutable images without depending on an installed JVM. */
#define IMAGE_SIZE (16u * 1024u * 1024u + 37u)
const unsigned char image_pages[IMAGE_SIZE] = {
    [0] = 3,
    [8u * 1024u * 1024u] = 91,
    [IMAGE_SIZE - 1] = 171,
};

int image_value(unsigned int index) {
    return image_pages[index % IMAGE_SIZE];
}
