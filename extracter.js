const got = require("got");
const cheerio = require("cheerio");

const extractLinks = async (url) => {
  try {
    const response = await got(url);
    const html = response.body;
    const $ = cheerio.load(html);
    const linkObjects = $("a");
    const links = [];
    linkObjects.each((index, element) => {
      links.push($(element).attr("href"));
    });
    return links;
  } catch (error) {
    return "urls not found";
  }
};

module.exports = {
    extractLinks: extractLinks,
};
