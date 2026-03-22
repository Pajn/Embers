window.openTab = function (evt, group, tab) {
  document
    .querySelectorAll('.tabcontent[group="' + group + '"]')
    .forEach(function (content) {
      content.style.display = "none";
    });

  document
    .querySelectorAll('.tablinks[group="' + group + '"]')
    .forEach(function (link) {
      link.classList.remove("active");
    });

  const target = document.getElementById(group + "-" + tab);
  if (target) {
    target.style.display = "block";
  }

  if (evt && evt.currentTarget) {
    evt.currentTarget.classList.add("active");
  }
};