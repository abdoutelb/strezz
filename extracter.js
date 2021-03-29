const got = require("got");
const cheerio = require("cheerio");

const extractLinks = async (url) => {
  try {
    // Fetching HTML
    const response = await got(url);
    const html = response.body;

    // Using cheerio to extract <a> tags
    const $ = cheerio.load(html);

    const linkObjects = $("a");
    // this is a mass object, not an array

    // Collect the "href" and "title" of each link and add them to an array
    const links = [];
    linkObjects.each((index, element) => {
      links.push($(element).attr("href"));
    });

    return links;
    // do something else here with these links, such as writing to a file or saving them to your database
  } catch (error) {
    console.log(error.response.body);
  }
};

try {
  module.exports = {
    extractLinks: extractLinks,
  };
} catch (error) {
  throw new Error("We caught error");
}
