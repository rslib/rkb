#include <cstdio>

int main() {
  std::vector<int> v{1, 2, 3};
  std::printf("%d\n", std::accumulate(v.begin(), v.end(), 0));
}
